# nvplay

A native Rust media player with a compact, VLC-inspired desktop interface for Windows, macOS, and Linux. Opens at **640 × 400**, with a black video surface and a compact single-row control strip. macOS uses the system menu bar; Windows and Linux keep an in-window menu bar. The playlist is hidden until requested. Built with **egui/eframe** and **libmpv**, with video rendered directly into an OpenGL surface inside the app.

This is a working foundation, not feature parity with VLC. Format support comes from the installed mpv/FFmpeg build: common containers and codecs such as MP4, MKV, WebM, MOV, AVI, H.264, HEVC, VP9, and AV1 are typically supported. DRM-protected services, corrupt files, and codecs omitted from your engine build are not guaranteed to play. OpenGL 3.3 or newer is required.

## Run

Install [Rust](https://rustup.rs), then install the playback runtime for your platform:

### macOS

```sh
brew install mpv yt-dlp deno
source "$HOME/.cargo/env"
cargo run --release
```

Homebrew's Apple Silicon and Intel library locations are detected automatically.

For a double-clickable local app, run `sh scripts/bundle-macos.sh` and open
`target/nvplay.app`. This unsigned bundle uses your installed libmpv; it is not
a standalone redistributable installer.

To install or replace the Applications copy, run:

```sh
sh scripts/bundle-macos.sh release
sh scripts/install-macos.sh
```

The installer stages and ad-hoc signs the bundle before replacing
`/Applications/nvplay.app`, restoring the previous app if replacement fails.

Choose **nvplay → Check for Updates…** (also under Help) to compare the running
build with this local source folder. **Build and Install Update** compiles a
release build and replaces the installed app in the background. **Restart Now**
activates the update. Help also offers **Automatically Install Updates**: when
enabled, nvplay checks on launch and every minute, builds changed source, and
installs it without interrupting playback. The preference is saved in
`~/Library/Application Support/nvplay/updates.conf`; automatic updates are off
by default. Rust, this source folder, and a writable app location are required.
This is a local development updater; it does not fetch GitHub releases or pull
remote source. A public release feed is not configured yet.

The macOS bundle declares video/audio document types, including MPEG-4. Use
Finder's **Open With → nvplay** to open files. A native Apple-event receiver
queues documents even during cold startup, then forwards them to the player;
opening another file while nvplay is running uses the same path. This does not
change your default media application.

### Windows

Install Rust's MSVC toolchain and Visual Studio C++ Build Tools. Obtain a **shared libmpv** build matching your architecture from the builds linked by [mpv's installation page](https://mpv.io/installation/). The standalone `mpv.exe` is not the library.

Put `mpv-2.dll` (or `libmpv-2.dll`) and its required runtime DLLs beside `target/release/nvplay.exe`, or set an absolute library path:

```powershell
$env:NVPLAY_MPV_LIBRARY = 'C:\mpv\mpv-2.dll'
cargo run --release
```

Keep dependent DLLs in a Windows loader search location, such as the executable directory or `PATH`.

### Linux

On recent Ubuntu/Debian:

```sh
sudo apt install build-essential pkg-config libmpv2 libxkbcommon-dev libwayland-dev libgl1-mesa-dev
cargo run --release
```

On Fedora, install the corresponding compiler, Wayland/XKB/OpenGL development packages and `mpv-libs`. On Arch, install `base-devel`, `mpv`, `libxkbcommon`, `wayland`, and `mesa`. Native file dialogs use the desktop portal on Linux; make sure your desktop's `xdg-desktop-portal` backend is installed and running.

`NVPLAY_MPV_LIBRARY` can point to a specific shared library on every platform. If it cannot load, the app displays the attempted paths and loader errors. No native library is needed at compile time because it is loaded at runtime.

### Open media directly

```sh
cargo run --release -- "/path/to/movie.mkv" "/path/to/another.mp4"
```

Use absolute paths for command-line files. File dialogs and drag-and-drop also accept files with any extension. Paths must be valid Unicode in this version.

### Website and live-stream playback

Choose **Media → Open Network Stream** (`Ctrl/Cmd+N`), paste a URL, and press Play. Public YouTube watch-page links and other sites supported by [yt-dlp](https://github.com/yt-dlp/yt-dlp) are resolved by mpv's built-in ytdl hook and played inside the video surface. Direct MP4/WebM URLs and unencrypted HLS/DASH or other supported network streams do not require yt-dlp. Playback streams media; it does not wait for a complete download. Web-page URLs go directly to the website extractor, while recognized media/playlist URLs retain mpv’s native streaming path.

The network dialog offers **Data saver (720p)**, **Balanced (1080p)** (default), and **Original quality**. Website selection prefers H.264 at the selected resolution when available, then other codecs at that resolution, with a best-available fallback for sites without matching formats. Direct HLS master playlists select a variant up to roughly 2.5 Mbps in Data saver or 6 Mbps in Balanced; Original selects the highest bitrate. HLS variant resolution depends on the server. This does not transcode direct URLs or reduce the resolution of a single fixed-quality stream. Network playback prebuffers about two seconds and reads ahead up to 30 seconds, with a 256 MiB forward-cache limit. These defaults trade a short initial buffer for fewer stalls.

Press **⌘⇧M** on macOS or **Ctrl+Shift+M** on Windows/Linux (or choose **Media → Search Movies…**) to search by movie title. Results appear as you type with poster thumbnails, titles, and release years. Posters load and decode in the background as results become visible, with at most four requests at once and a 64-poster memory cache. Missing artwork uses a placeholder and never blocks selection or playback. Use ↑/↓ to select a result and Enter to play, or click a result. Enter while results are loading plays the first verified match when ready. Esc closes search without interrupting the current movie. TMDB supplies candidate titles and artwork, but candidates are hidden until Cinejoy availability is verified. Up to three background checks resolve a movie, fetch the selected video/audio HLS playlists, and probe the initialization/first media bytes; failed or unchecked candidates never enter the result list. Confirmed matches appear progressively while remaining candidates are checked, with a 60-second overall search limit. Positive checks are cached for two minutes (up to 256 IDs); playback resolves afresh to avoid reusing expired stream URLs. Closing or editing search cancels outstanding checks. No website navigation or browser session is involved. Availability can still change after verification; a server outage or expired source can cause a later playback failure. Other website providers are not yet integrated into title search. Deno is required; Cinejoy's public catalog credential/API may change and require an integration update. TMDB attribution is available in the dialog's Movie data credits.

Cinejoy movie watch URLs (`https://cinejoy.pk/watch/movie/<id>`) use a dedicated resolver instead of yt-dlp. Deno runs the public request encoder with a verified SHA-256 hash and no host imports, resolves HLS through the current API, and passes a temporary playlist to mpv. No browser or advertisements are loaded. The chosen video quality and default audio are selected before playback so FFmpeg does not probe every language track. This initial adapter supports movies, one audio track, and the existing 720p/1080p/original quality setting; TV episodes, additional audio selection, and subtitle importing are not implemented. Stop, opening another file, and closing the app cancel resolution. Resolution is limited to 35 seconds; upstream API/encoder changes may require an adapter update. Temporary playlists are removed when replaced or the player closes.

**View → Playback information** shows buffered seconds, read rate, codec, active hardware decoder, and dropped frames. Low buffered time suggests a network/server bottleneck; accumulating decoder or render drops suggests a decoding/rendering bottleneck. Try Data saver when a website stream is too heavy.

Install a current yt-dlp build and its recommended JavaScript runtime, Deno, for YouTube. On Windows, place `yt-dlp.exe` and `deno.exe` beside `nvplay.exe`, or install them on `PATH`. On Linux, install current versions using the official projects' installation instructions. Some distribution packages lag behind website changes. Official yt-dlp builds should include the matching EJS components; see [yt-dlp's JavaScript requirements](https://github.com/yt-dlp/yt-dlp/wiki/EJS).

The app discovers tools beside the executable, in a macOS bundle's `Contents/Resources`, on `PATH`, and in Homebrew's standard locations. Override with absolute executable paths in `NVPLAY_YTDLP` and `NVPLAY_DENO`. This also supports launching the macOS app from Finder with a minimal `PATH`. If a tool was just installed, restart nvplay. The libmpv build must include its Lua/ytdl hook support.

**Netflix does not play here.** Its streams require account authentication and DRM through a [Netflix-supported browser or app](https://help.netflix.com/en/node/30081). Netflix links show an explicit explanation in the URL dialog. Other DRM services are also unsupported. Private, age-restricted, region-locked, or expired videos may fail; the app does not read your browser cookies or sign in to websites. Errors include recent engine/extractor diagnostics, and Stop cancels a pending stream. Website support depends on the current extractor and service, so arbitrary URLs cannot be guaranteed. A JavaScript-only watch page may have no extractable video in its HTML and needs a site-specific extractor. Unsupported pages report **Website not supported** with the extractor reason; follow-on errors about the internal `ytdl://` URL are suppressed.

## Included

- Embedded video with aspect-ratio preservation and engine-managed subtitles.
- Native macOS menus for media, playback, tracks, audio devices, subtitles, view, and help; in-window menus on Windows and Linux. Painted toolbar icons and an edge-to-edge video area.
- Open multiple files, drag-and-drop, command-line files, or network URLs.
- Queue selection, removal, previous/next, repeat, and automatic advance at EOF.
- Play/pause/stop, seek, volume/mute, and playback speed.
- Named audio, video, and subtitle track selection; external subtitle files.
- Audio output device selection and engine-synchronized mute/volume controls.
- A taller timeline aligned with the control buttons, click/drag seeking, wheel/trackpad seeking, and hover frame previews for local videos. Previews use FFmpeg in a separate background process and do not seek the playing video. Install `ffmpeg` (macOS: `brew install ffmpeg`), bundle it beside nvplay or in `Contents/Resources`, or set `NVPLAY_FFMPEG` to its executable. Network streams retain timestamp previews.
- Fullscreen controls revealed by pointer movement; controls stay visible while sliders are in use. macOS menus remain in the system menu bar, revealed at the top screen edge in fullscreen. Showing controls does not resize the video.
- Accessible buttons/sliders and keyboard shortcuts.
- Asynchronous playback commands and visible playback/load errors.

| Shortcut | Action |
| --- | --- |
| Ctrl/Cmd + O | Open files |
| Ctrl/Cmd + N | Open network stream |
| Ctrl/Cmd + Shift + M | Search and play movies |
| Ctrl/Cmd + L | Show/hide playlist |
| Space | Play / pause |
| Left / Right | Seek five seconds |
| Up / Down | Change volume |
| M | Toggle mute |
| F or double-click video | Toggle fullscreen |
| Escape | Leave fullscreen |

## Structure

```text
src/main.rs              Native window and startup
src/app.rs               UI, queue, file dialogs, input handling
src/app/icons.rs         Small toolbar icons and empty-state cone
src/streaming.rs         Network URL validation and yt-dlp/Deno discovery
src/open_documents.rs    Finder open-document delivery, including cold startup
src/playback/mod.rs      Playback commands, state, engine events
src/playback/render.rs   OpenGL render context and video framebuffer
src/playback/ffi.rs      Small, dynamically loaded libmpv ABI surface
src/playback/properties.rs  Track and audio-device property decoding
```

Keep future features behind the playback layer rather than adding native API calls to the UI. The engine uses asynchronous commands and observed properties. Engine events and new-video-frame callbacks wake the UI immediately. Active video also repaints continuously at eframe’s vsync cadence; paused/idle playback stays event-driven. The engine uses `video-sync=audio` and `video-timing-offset=0`, so its nonblocking render callback receives frames at their due time instead of presenting them ahead of the audio clock. All render calls stay on the GL thread. Only new frames or surface resizes invoke mpv's renderer; hover and menu repaints reuse the existing video texture. A separate framebuffer and clipping preserve the surrounding controls. The render context and callbacks are released before the window's OpenGL context or engine handle.

Mute, selected tracks, volume, duration, position, seekability, buffering, and audio output all come from engine properties. A seek/volume preview lasts only until the engine acknowledges it or a short timeout expires. Audio metadata is refreshed asynchronously after decoder reconfiguration. Native file pickers use non-blocking futures polled on the UI thread. **View → Playback information** shows current engine state, audio format/output, and render statistics for troubleshooting.

Useful next milestones: persistent settings and recent files, frame stepping and screenshots, playlist import/export, platform-specific packaging, and signed installers. Queue and settings are currently in-memory. HDR passthrough, disc menus, casting, and a plugin system are not implemented. On macOS the OpenGL player prefers VideoToolbox copy-back hardware decoding, with mpv auto/software fallback. Other platforms use mpv’s `auto-safe` policy. This does not guarantee zero-copy decoding.

## Validate

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

An explicit integration test exercises the real engine without a display or sound device. With FFmpeg installed:

```sh
ffmpeg -f lavfi -i 'testsrc2=size=960x540:rate=30' -t 12 -c:v libx264 -pix_fmt yuv420p /tmp/nvplay-test.mp4
NVPLAY_TEST_MEDIA=/tmp/nvplay-test.mp4 cargo test real_engine -- --ignored
```

This checks loading, duration, pause, exact seek, volume, and asynchronous command errors. It does not validate GPU output or audio devices. Run the app with actual media on each target OS before shipping. CI checks compilation, lint, and unit tests on all three desktop platforms; platform installers and native playback testing remain release work.

For an actual UI-to-engine regression check, with FFmpeg and libmpv installed:

```sh
sh scripts/check-playback.sh
```

This creates a temporary 30-second fixture with English/French audio and subtitles. It sends real egui pointer presses, moves, and releases to the toolbar and sliders; checks that the accessible timeline advances; pauses; previews a frame without moving playback; scroll-seeks and drags a seek while paused; changes volume and mute; selects French audio through the menu action (native menu model on macOS); opens the URL dialog from Media; and stops playback. It verifies engine state and accessible widget values, rather than checking appearance alone. Normal unit tests verify native macOS menu actions and checkmarks and the absence of an in-window menu; other platforms verify that a fullscreen menu stays open away from the bar and that overlay controls do not resize the video. These checks do not measure audible sound quality or replace native GPU/input testing.

To verify real network playback (including website extraction), run:

```sh
NVPLAY_TEST_URL='https://www.youtube.com/watch?v=jNQXAC9IVRw' cargo test network_engine -- --ignored
```

This waits for a loaded stream, checks that playback advances past three seconds, and verifies Stop returns the engine to idle. Public videos and services can change; this test is opt-in and requires network access. It can also run against a local HTTP server serving a progressive MP4 (with `-movflags +faststart`) or a server supporting byte ranges. Unit tests cover invalid URLs, website/direct-stream routing, useful extractor errors, Netflix host detection, and mpv option quoting for tool paths.

For repeatable local streaming checks (FFmpeg, yt-dlp, and Python 3 required):

```sh
python3 scripts/check-streaming.py
cargo test --locked real_quality -- --ignored
```

These verify progressive HTTP, an HTML5 video web page resolved through yt-dlp, HLS including bitrate-based variant selection, extractor error reporting, and 720p/1080p/original format selection. They run without opening the app or an audio/video output device. They do not measure GPU smoothness or guarantee support for arbitrary websites.

For actual GPU rendering and frame timing on macOS:

```sh
sh scripts/check-rendering-macos.sh
```

This generates 24 fps, 60 fps, and one-second clips. It runs both the renderer
alone and the complete egui UI/paint path in an offscreen CGL context, including
the real macOS audio clock with output muted. It compares media progress with
wall time, checks that the short clip finishes at normal speed, and verifies
frame deadlines, paused texture reuse, and a paused seek changing rendered
pixels. A normal unit test also ensures fractional playback timestamps never
trigger an unsolicited seek on repaint. It opens no windows and makes no sound.
It does not measure compositor/display latency or replace visually checking
the affected file on your display.

## Distribution and licensing

The Rust application is GPL-3.0-or-later. libmpv and codec dependencies have their own licenses; mpv's default build is GPL, with LGPL builds possible under specific build options. Distribute the correct native libraries, license notices, and corresponding source as required by the exact builds you bundle. Codec patent obligations depend on distribution and jurisdiction. See [mpv's copyright information](https://github.com/mpv-player/mpv/blob/master/Copyright).

API references: [libmpv client API](https://github.com/mpv-player/mpv/blob/master/include/mpv/client.h), [render API](https://github.com/mpv-player/mpv/blob/master/include/mpv/render.h), [OpenGL integration](https://github.com/mpv-player/mpv/blob/master/include/mpv/render_gl.h), and [eframe](https://docs.rs/eframe/0.33.3/eframe/).

To verify Cinejoy end to end without opening a window or audio device:

```sh
NVPLAY_TEST_URL=https://cinejoy.pk/watch/movie/210577 cargo test --locked cinejoy_movie_resolves_seeks_and_cancels -- --ignored --nocapture
```

This checks resolution cancellation, 1080p audio/video playback, pause, a seek to 60 seconds, resumed playback, and temporary-playlist cleanup. It requires Deno, libmpv, network access, and an available upstream movie. The request protocol was cross-checked against [Pandaflix's GPL-3.0 Cinejoy implementation](https://github.com/MADPANDA3D/pandaflix/blob/master/core/providers/cinejoy.go); nvplay uses the endpoint and encoder observed on the current Cinejoy page.

To check the complete search-to-play flow without controlling the desktop:

```sh
cargo test --locked real_search_dialog_starts_movie_and_preserves_title -- --ignored --nocapture
```

It opens the dialog through its shortcut, types a title, presses Enter once, resolves the returned Cinejoy movie, waits for playback to advance, and checks the displayed title. Unit checks cover both Command and Control shortcut events, focus, keyboard/click selection, stale-result clearing, one-letter titles, and Escape while fullscreen. Windows shortcuts and hidden helper processes are implemented, but the current local runtime checks were performed on macOS.

The application icon is sourced from `src/app/assets/nvplay.svg`. To change it,
replace that SVG, run `sh scripts/generate-icons-macos.sh`, then rebuild the app.
The generated PNG is embedded for window icons, and the macOS bundler installs
`nvplay.icns` into `Contents/Resources` and declares it in Info.plist. Local app
updates retain this bundled icon.
