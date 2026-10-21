mod ffi;
mod properties;
mod render;
pub use properties::{AudioDevice, Track};
pub use render::VideoRenderer;
#[cfg(all(test, target_os = "macos"))]
pub(crate) use render::tests::Offscreen;

use crate::streaming;
use ffi::{Api, EndFile, LogMessage, Node, Property};
use std::{
    collections::VecDeque,
    ffi::{CStr, CString, c_void},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Default, Debug)]
pub struct PlaybackState {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub volume: f64,
    pub speed: f64,
    pub title: String,
    pub idle: bool,
    pub buffering: bool,
    pub muted: bool,
    pub seekable: bool,
    pub seeking: bool,
    pub subtitles_visible: bool,
    pub tracks: Vec<Track>,
    pub audio_devices: Vec<AudioDevice>,
    pub audio_device: String,
    pub audio_track: String,
    pub subtitle_track: String,
    pub audio_codec: String,
    pub audio_channels: i64,
    pub cache_seconds: f64,
    pub cache_bytes_per_second: i64,
    pub hardware_decoder: String,
    pub video_codec: String,
    pub video_width: i64,
    pub video_height: i64,
    pub dropped_frames: i64,
    pub decoder_dropped_frames: i64,
}

pub enum PlaybackEvent {
    Finished,
    Error(String),
    Loaded,
}

pub struct Player {
    pub state: PlaybackState,
    client: Arc<Client>,
    diagnostics: VecDeque<String>,
    source: String,
    pending_load_error: Option<(String, Instant)>,
    extractor_available: bool,
    cinejoy_resolver: Option<streaming::cinejoy::Resolver>,
    cinejoy_playlist: Option<streaming::cinejoy::Playlist>,
    pub stream_quality: streaming::Quality,
}

struct Client {
    api: Arc<Api>,
    handle: *mut c_void,
    wakeup: Option<Box<eframe::egui::Context>>,
}
// SAFETY: libmpv client handles support concurrent API use. The renderer holds
// an Arc to keep this handle alive; rendering itself stays on the GL thread.
unsafe impl Send for Client {}
unsafe impl Sync for Client {}
impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the last Arc drops only after the render context is freed.
        unsafe {
            (self.api.set_wakeup_callback)(self.handle, None, std::ptr::null_mut());
            (self.api.terminate_destroy)(self.handle);
        }
    }
}

impl Player {
    #[cfg(test)]
    pub(crate) fn for_ui_test(ctx: &eframe::egui::Context) -> Result<Self, String> {
        Self::create("null", Some("null"), Some(ctx))
    }
    pub fn new(ctx: &eframe::egui::Context) -> Result<Self, String> {
        Self::create("libmpv", None, Some(ctx))
    }

    #[cfg(test)]
    fn with_output(video: &str, audio: Option<&str>) -> Result<Self, String> {
        Self::create(video, audio, None)
    }

    pub(crate) fn create(
        video: &str,
        audio: Option<&str>,
        ctx: Option<&eframe::egui::Context>,
    ) -> Result<Self, String> {
        let api = Api::load()?;
        // SAFETY: the returned handle is checked and is owned by Client.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("Could not create the mpv playback engine".into());
        }
        let client = Arc::new(Client {
            api,
            handle,
            wakeup: ctx.map(|c| Box::new(c.clone())),
        });
        for (key, value) in [
            ("vo", video),
            (
                "hwdec",
                if cfg!(target_os = "macos") {
                    "videotoolbox-copy,auto"
                } else {
                    "auto-safe"
                },
            ),
            ("idle", "yes"),
            ("keep-open", "no"),
            ("terminal", "no"),
            ("input-default-bindings", "no"),
            ("osc", "no"),
            // Eframe owns presentation and must not block on mpv's render call.
            // Queue frames at their due time instead of receiving them early
            // and displaying them without the render API's normal timing wait.
            ("video-sync", "audio"),
            ("video-timing-offset", "0"),
            ("volume", "70"),
            ("ytdl", "yes"),
            ("load-scripts", "no"),
            ("network-timeout", "20"),
            ("cache", "auto"),
            ("cache-secs", "30"),
            ("demuxer-max-bytes", "256MiB"),
            ("demuxer-max-back-bytes", "32MiB"),
            ("cache-pause-wait", "2"),
            ("ytdl-format", streaming::Quality::default().format()),
        ] {
            client.option(key, value)?;
        }
        if let Some(audio) = audio {
            client.option("ao", audio)?;
        }
        #[cfg(test)]
        if audio == Some("coreaudio") {
            // Exercise the real macOS audio clock without making any sound.
            client.option("mute", "yes")?;
        }
        let tools = streaming::Tools::discover();
        let extractor_available = tools.ytdlp.is_some();
        let mut script_options = vec!["ytdl_hook-all_formats=no".to_owned()];
        if let Some(path) = tools.ytdlp {
            script_options.push(streaming::option_value(
                "ytdl_hook-ytdl_path",
                &path.to_string_lossy(),
            ));
        }
        client.option("script-opts", &script_options.join(","))?;
        if let Some(path) = tools.deno {
            client.option(
                "ytdl-raw-options",
                &streaming::option_value(
                    "js-runtimes",
                    &format!("deno:{}", path.to_string_lossy()),
                ),
            )?;
        }
        // SAFETY: configured, not-yet-initialized live handle.
        client
            .api
            .check(unsafe { (client.api.initialize)(handle) })?;
        // SAFETY: live handle; mpv copies the static log level string.
        client
            .api
            .check(unsafe { (client.api.request_log_messages)(handle, c"warn".as_ptr()) })?;
        for (index, (name, format)) in [
            ("time-pos", 5),
            ("duration", 5),
            ("pause", 3),
            ("volume", 5),
            ("speed", 5),
            ("media-title", 1),
            ("idle-active", 3),
            ("paused-for-cache", 3),
            ("mute", 3),
            ("seekable", 3),
            ("seeking", 3),
            ("sub-visibility", 3),
            ("track-list", 6),
            ("audio-device-list", 6),
            ("audio-device", 1),
            ("aid", 1),
            ("sid", 1),
            ("audio-codec-name", 1),
            ("audio-params/channel-count", 4),
            ("demuxer-cache-duration", 5),
            ("cache-speed", 4),
            ("hwdec-current", 1),
            ("video-codec", 1),
            ("width", 4),
            ("height", 4),
            ("frame-drop-count", 4),
            ("decoder-frame-drop-count", 4),
        ]
        .iter()
        .enumerate()
        {
            let name = CString::new(*name).unwrap();
            // SAFETY: the API copies the property name before returning.
            client.api.check(unsafe {
                (client.api.observe_property)(handle, index as u64, name.as_ptr(), *format)
            })?;
        }
        if let Some(wakeup) = &client.wakeup {
            // SAFETY: boxed Context is stable and retained until after mpv's
            // threads shut down. The callback only schedules a UI repaint.
            unsafe {
                (client.api.set_wakeup_callback)(
                    handle,
                    Some(wake_ui),
                    (&**wakeup as *const eframe::egui::Context)
                        .cast_mut()
                        .cast(),
                )
            };
        }
        Ok(Self {
            client,
            diagnostics: VecDeque::new(),
            source: String::new(),
            pending_load_error: None,
            extractor_available,
            cinejoy_resolver: None,
            cinejoy_playlist: None,
            stream_quality: streaming::Quality::default(),
            state: PlaybackState {
                volume: 70.0,
                speed: 1.0,
                idle: true,
                ..Default::default()
            },
        })
    }

    /// Commands are asynchronous so slow network input cannot block the UI or
    /// deadlock against mpv's render thread. Completion errors arrive via poll.
    pub fn command(&mut self, arguments: &[&str]) -> Result<(), String> {
        if arguments.first() == Some(&"stop") {
            self.cinejoy_resolver = None;
            self.pending_load_error = None;
        }
        let strings: Vec<CString> = arguments
            .iter()
            .map(|s| {
                CString::new(*s)
                    .map_err(|_| "Media paths and commands cannot contain a NUL byte".to_string())
            })
            .collect::<Result<_, _>>()?;
        let mut pointers: Vec<_> = strings.iter().map(|s| s.as_ptr()).collect();
        pointers.push(std::ptr::null());
        // SAFETY: null-terminated argument array valid for this call; mpv copies it.
        self.client.api.check(unsafe {
            (self.client.api.command_async)(
                self.client.handle,
                u64::from(arguments.first() == Some(&"loadfile")),
                pointers.as_ptr(),
            )
        })
    }

    pub fn load(&mut self, source: &str) -> Result<(), String> {
        streaming::check_service(source)?;
        let cinejoy_id = streaming::cinejoy::movie_id(source);
        let resolver = if cinejoy_id.is_some() {
            Some(streaming::cinejoy::Resolver::start(
                source,
                self.stream_quality,
            )?)
        } else {
            None
        };
        self.command(&["stop"])?;
        self.cinejoy_playlist = None;
        self.command(&["set", "referrer", ""])?;
        self.command(&["set", "http-header-fields", ""])?;
        self.command(&["set", "demuxer-lavf-o", ""])?;
        self.command(&["set", "force-media-title", ""])?;
        let playback_source = streaming::playback_source(source, self.extractor_available);
        self.command(&["set", "ytdl-format", self.stream_quality.format()])?;
        self.command(&["set", "hls-bitrate", self.stream_quality.hls_bitrate()])?;
        self.command(&[
            "set",
            "cache-pause-initial",
            if streaming::is_network(source) {
                "yes"
            } else {
                "no"
            },
        ])?;
        if let Some(id) = cinejoy_id {
            self.command(&["set", "force-media-title", &format!("Cinejoy movie {id}")])?;
            self.cinejoy_resolver = resolver;
            if let Some(ctx) = &self.client.wakeup {
                ctx.request_repaint();
            }
        } else {
            self.command(&["loadfile", &playback_source, "replace"])?;
        }
        self.source = source.into();
        self.pending_load_error = None;
        self.command(&["set", "pause", "no"])?;
        self.state.position = 0.0;
        self.state.duration = 0.0;
        self.state.title.clear();
        self.state.cache_seconds = 0.0;
        self.state.cache_bytes_per_second = 0;
        self.state.dropped_frames = 0;
        self.state.decoder_dropped_frames = 0;
        self.state.video_codec.clear();
        self.state.video_width = 0;
        self.state.video_height = 0;
        self.state.hardware_decoder.clear();
        self.diagnostics.clear();
        Ok(())
    }

    pub fn poll(&mut self) -> Vec<PlaybackEvent> {
        let mut events = Vec::new();
        // Bound work per frame so a noisy input cannot starve the UI.
        for _ in 0..256 {
            // SAFETY: the event remains valid until the next wait_event call.
            let event = unsafe { &*(self.client.api.wait_event)(self.client.handle, 0.0) };
            match event.id {
                0 => break,
                2 if !event.data.is_null() => {
                    // SAFETY: event 2 contains an mpv_event_log_message. Copy text
                    // before the next wait_event invalidates its pointers.
                    let log = unsafe { &*event.data.cast::<LogMessage>() };
                    if !log.text.is_null() && !log.prefix.is_null() {
                        let prefix = unsafe { CStr::from_ptr(log.prefix) }.to_string_lossy();
                        let message = unsafe { CStr::from_ptr(log.text) }.to_string_lossy();
                        if log.log_level <= 20 || prefix.contains("ytdl") {
                            self.diagnostics.push_back(format!(
                                "{prefix}: {}",
                                message.trim().chars().take(500).collect::<String>()
                            ));
                            if self.diagnostics.len() > 8 {
                                self.diagnostics.pop_front();
                            }
                        }
                    }
                }
                5 if event.error < 0 => {
                    let summary = self.client.api.check(event.error).unwrap_err();
                    if event.userdata == 1 {
                        self.pending_load_error = Some((summary, Instant::now()));
                    } else {
                        events.push(PlaybackEvent::Error(summary));
                    }
                }
                7 if !event.data.is_null() => {
                    // SAFETY: event 7 payload begins with reason and error integers.
                    let end = unsafe { &*event.data.cast::<EndFile>() };
                    if end.reason == 0 {
                        events.push(PlaybackEvent::Finished);
                    }
                    if end.reason == 4 {
                        let summary = self
                            .client
                            .api
                            .check(end.error)
                            .err()
                            .unwrap_or_else(|| "Playback failed".into());
                        self.pending_load_error = Some((summary, Instant::now()));
                    }
                }
                8 => events.push(PlaybackEvent::Loaded),
                18 => {
                    // A decoder/output rebuild can temporarily invalidate audio
                    // properties. Refresh after reconfiguration without waiting
                    // synchronously on the core from the render/UI thread.
                    unsafe {
                        (self.client.api.get_property_async)(
                            self.client.handle,
                            0,
                            c"audio-params/channel-count".as_ptr(),
                            4,
                        );
                        (self.client.api.get_property_async)(
                            self.client.handle,
                            0,
                            c"audio-codec-name".as_ptr(),
                            1,
                        );
                    }
                }
                3 | 22 if !event.data.is_null() => {
                    // SAFETY: property event data follows the observed format. Copy
                    // all values before asking mpv for the next event.
                    let prop = unsafe { &*event.data.cast::<Property>() };
                    let name = unsafe { CStr::from_ptr(prop.name) }.to_bytes();
                    if prop.data.is_null() {
                        match name {
                            b"time-pos" => self.state.position = 0.0,
                            b"duration" => self.state.duration = 0.0,
                            b"demuxer-cache-duration" => self.state.cache_seconds = 0.0,
                            b"cache-speed" => self.state.cache_bytes_per_second = 0,
                            b"hwdec-current" => self.state.hardware_decoder.clear(),
                            b"video-codec" => self.state.video_codec.clear(),
                            b"width" => self.state.video_width = 0,
                            b"height" => self.state.video_height = 0,
                            b"frame-drop-count" => self.state.dropped_frames = 0,
                            b"decoder-frame-drop-count" => self.state.decoder_dropped_frames = 0,
                            b"track-list" => {
                                self.state.tracks.clear();
                                self.state.audio_channels = 0;
                                self.state.audio_codec.clear();
                            }
                            b"audio-codec-name" => {
                                self.state.audio_codec = self
                                    .state
                                    .tracks
                                    .iter()
                                    .find(|t| t.kind == "audio" && t.selected)
                                    .map_or_else(String::new, |t| t.codec.clone())
                            }
                            b"audio-params/channel-count" => {
                                // Decoder properties can be temporarily unavailable
                                // during a track rebuild. File track metadata is
                                // still valid; don't flash "0 channels" in the UI.
                                self.state.audio_channels = self
                                    .state
                                    .tracks
                                    .iter()
                                    .find(|t| t.kind == "audio" && t.selected)
                                    .map_or(0, |t| t.channels);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    match (name, prop.format) {
                        (b"width", 4) => {
                            self.state.video_width = unsafe { *prop.data.cast::<i64>() }
                        }
                        (b"height", 4) => {
                            self.state.video_height = unsafe { *prop.data.cast::<i64>() }
                        }
                        (b"demuxer-cache-duration", 5) => {
                            self.state.cache_seconds = unsafe { *prop.data.cast::<f64>() }
                        }
                        (b"cache-speed", 4) => {
                            self.state.cache_bytes_per_second = unsafe { *prop.data.cast::<i64>() }
                        }
                        (b"frame-drop-count", 4) => {
                            self.state.dropped_frames = unsafe { *prop.data.cast::<i64>() }
                        }
                        (b"decoder-frame-drop-count", 4) => {
                            self.state.decoder_dropped_frames = unsafe { *prop.data.cast::<i64>() }
                        }
                        (b"time-pos", 5) => {
                            self.state.position = unsafe { *prop.data.cast::<f64>() }
                        }
                        (b"duration", 5) => {
                            self.state.duration = unsafe { *prop.data.cast::<f64>() }
                        }
                        (b"volume", 5) => self.state.volume = unsafe { *prop.data.cast::<f64>() },
                        (b"speed", 5) => self.state.speed = unsafe { *prop.data.cast::<f64>() },
                        (b"pause", 3) => {
                            self.state.paused = unsafe { *prop.data.cast::<i32>() != 0 }
                        }
                        (b"idle-active", 3) => {
                            self.state.idle = unsafe { *prop.data.cast::<i32>() != 0 }
                        }
                        (b"paused-for-cache", 3) => {
                            self.state.buffering = unsafe { *prop.data.cast::<i32>() != 0 }
                        }
                        (b"mute", 3) => self.state.muted = unsafe { *prop.data.cast::<i32>() != 0 },
                        (b"seekable", 3) => {
                            self.state.seekable = unsafe { *prop.data.cast::<i32>() != 0 }
                        }
                        (b"seeking", 3) => {
                            self.state.seeking = unsafe { *prop.data.cast::<i32>() != 0 }
                        }
                        (b"sub-visibility", 3) => {
                            self.state.subtitles_visible = unsafe { *prop.data.cast::<i32>() != 0 }
                        }
                        (b"track-list", 6) => {
                            self.state.tracks =
                                unsafe { properties::tracks(&*prop.data.cast::<Node>()) };
                            if let Some(track) = self
                                .state
                                .tracks
                                .iter()
                                .find(|t| t.kind == "audio" && t.selected)
                            {
                                self.state.audio_channels = track.channels;
                                self.state.audio_codec.clone_from(&track.codec);
                            } else {
                                self.state.audio_channels = 0;
                                self.state.audio_codec.clear();
                            }
                        }
                        (b"audio-device-list", 6) => {
                            self.state.audio_devices =
                                unsafe { properties::devices(&*prop.data.cast::<Node>()) }
                        }
                        (b"audio-params/channel-count", 4) => {
                            self.state.audio_channels = unsafe { *prop.data.cast::<i64>() }
                        }
                        (
                            b"audio-device" | b"aid" | b"sid" | b"audio-codec-name"
                            | b"hwdec-current" | b"video-codec",
                            1,
                        ) => {
                            let string = unsafe { *prop.data.cast::<*const std::ffi::c_char>() };
                            let value = if string.is_null() {
                                String::new()
                            } else {
                                unsafe { CStr::from_ptr(string) }
                                    .to_string_lossy()
                                    .into_owned()
                            };
                            match name {
                                b"audio-device" => self.state.audio_device = value,
                                b"aid" => self.state.audio_track = value,
                                b"sid" => self.state.subtitle_track = value,
                                b"hwdec-current" => self.state.hardware_decoder = value,
                                b"video-codec" => self.state.video_codec = value,
                                _ => self.state.audio_codec = value,
                            }
                        }
                        (b"media-title", 1) => {
                            let string = unsafe { *prop.data.cast::<*const std::ffi::c_char>() };
                            if !string.is_null() {
                                self.state.title = unsafe { CStr::from_ptr(string) }
                                    .to_string_lossy()
                                    .into_owned();
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        if let Some(resolver) = &mut self.cinejoy_resolver {
            if let Some(result) = resolver.poll() {
                self.cinejoy_resolver = None;
                let result = result.and_then(|playlist| {
                    let url = url::Url::parse(&self.source).map_err(|e| e.to_string())?;
                    let origin = url.origin().ascii_serialization();
                    self.command(&["set", "referrer", &format!("{origin}/")])?;
                    self.command(&["set", "http-header-fields", &format!("Origin: {origin}")])?;
                    // This generated local master references HTTPS media. FFmpeg
                    // otherwise restricts local playlists to file/crypto/data.
                    self.command(&[
                        "set",
                        "demuxer-lavf-o",
                        &streaming::option_value(
                            "protocol_whitelist",
                            "file,http,https,tcp,tls,crypto,data",
                        ),
                    ])?;
                    self.command(&["loadfile", playlist.path(), "replace"])?;
                    self.cinejoy_playlist = Some(playlist);
                    Ok(())
                });
                if let Err(error) = result {
                    events.push(PlaybackEvent::Error(error));
                }
            } else if let Some(ctx) = &self.client.wakeup {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        // Extractor stderr and load-failure events can arrive in either order.
        // Wait briefly without blocking the UI, then attach the final reason.
        if let Some((_, since)) = &self.pending_load_error {
            let remaining = Duration::from_millis(150).saturating_sub(since.elapsed());
            if remaining.is_zero() {
                let (summary, _) = self.pending_load_error.take().unwrap();
                let detail = self
                    .diagnostics
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                events.push(PlaybackEvent::Error(streaming::playback_error(
                    &self.source,
                    self.extractor_available,
                    &summary,
                    &detail,
                )));
            } else if let Some(ctx) = &self.client.wakeup {
                ctx.request_repaint_after(remaining);
            }
        }
        events
    }
}
unsafe extern "C" fn wake_ui(data: *mut c_void) {
    // SAFETY: Client retains the boxed Context until engine shutdown completes.
    unsafe { &*data.cast::<eframe::egui::Context>() }.request_repaint();
}
impl Client {
    fn option(&self, name: &str, value: &str) -> Result<(), String> {
        let (name, value) = (CString::new(name).unwrap(), CString::new(value).unwrap());
        // SAFETY: live uninitialized handle and valid C strings.
        self.api.check(unsafe {
            (self.api.set_option_string)(self.handle, name.as_ptr(), value.as_ptr())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "requires libmpv and NVPLAY_TEST_URL; website URLs also require yt-dlp"]
    fn network_engine_loads_advances_and_stops() {
        let source = std::env::var("NVPLAY_TEST_URL").expect("set NVPLAY_TEST_URL");
        let mut player = Player::with_output("null", Some("null")).unwrap();
        player.stream_quality = match std::env::var("NVPLAY_TEST_QUALITY").as_deref() {
            Ok("720") => streaming::Quality::DataSaver,
            Ok("original") => streaming::Quality::Original,
            _ => streaming::Quality::Balanced,
        };
        player
            .load(&streaming::network_url(&source).unwrap())
            .unwrap();
        let started = Instant::now();
        let deadline = started + Duration::from_secs(90);
        let mut loaded = false;
        loop {
            for event in player.poll() {
                match event {
                    PlaybackEvent::Loaded => loaded = true,
                    PlaybackEvent::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
            if loaded && player.state.position > 3.0 {
                break;
            }
            assert!(Instant::now() < deadline, "network media did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!(
            "Network playback advanced past 3 seconds in {:.2}s; cache {:.1}s, {} bytes/s",
            started.elapsed().as_secs_f64(),
            player.state.cache_seconds,
            player.state.cache_bytes_per_second
        );
        assert!(!player.state.idle);
        if let Ok(height) = std::env::var("NVPLAY_TEST_HEIGHT") {
            assert_eq!(
                player.state.video_height,
                height.parse::<i64>().unwrap(),
                "wrong HLS quality variant"
            );
        }
        player.command(&["stop"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.state.idle && Instant::now() < deadline {
            player.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(player.state.idle, "stop did not return the engine to idle");
    }

    #[test]
    #[ignore = "requires libmpv, Deno and a working Cinejoy movie URL in NVPLAY_TEST_URL"]
    fn cinejoy_movie_resolves_seeks_and_cancels() {
        let source = std::env::var("NVPLAY_TEST_URL").unwrap();
        assert!(streaming::cinejoy::movie_id(&source).is_some());
        let mut player = Player::with_output("null", Some("null")).unwrap();
        // Stop during resolution must not allow a later result to start media.
        player.load(&source).unwrap();
        assert!(player.cinejoy_resolver.is_some());
        player.command(&["stop"]).unwrap();
        assert!(player.cinejoy_resolver.is_none());
        player.load(&source).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            for event in player.poll() {
                if let PlaybackEvent::Error(error) = event {
                    panic!("{error}");
                }
            }
            if player.state.position > 1.0 {
                break;
            }
            assert!(Instant::now() < deadline, "Cinejoy did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(player.state.video_height, 1080);
        for kind in ["audio", "video"] {
            assert!(
                player
                    .state
                    .tracks
                    .iter()
                    .any(|t| t.kind == kind && t.selected),
                "missing {kind}"
            );
        }
        player.command(&["set", "pause", "yes"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !player.state.paused {
            player.poll();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        player.command(&["seek", "60", "absolute+exact"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while (player.state.position - 60.0).abs() > 1.0 || player.state.seeking {
            for event in player.poll() {
                if let PlaybackEvent::Error(error) = event {
                    panic!("{error}");
                }
            }
            assert!(Instant::now() < deadline, "Cinejoy seek did not complete");
            std::thread::sleep(Duration::from_millis(20));
        }
        player.command(&["set", "pause", "no"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while player.state.position < 62.0 {
            for event in player.poll() {
                if let PlaybackEvent::Error(error) = event {
                    panic!("{error}");
                }
            }
            assert!(
                Instant::now() < deadline,
                "Cinejoy did not resume after seeking"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let playlist = std::path::PathBuf::from(player.cinejoy_playlist.as_ref().unwrap().path());
        drop(player);
        assert!(!playlist.exists(), "temporary playlist was not removed");
    }

    /// Generate a local clip first; see README. Kept explicit because CI builds
    /// the UI without requiring an installed libmpv or an audio device.
    #[test]
    #[ignore = "requires libmpv, yt-dlp and NVPLAY_TEST_URL pointing to an HTML page without a video"]
    fn network_unsupported_page_reports_extractor_reason() {
        let source = std::env::var("NVPLAY_TEST_URL").unwrap();
        let mut player = Player::with_output("null", Some("null")).unwrap();
        player.load(&source).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for event in player.poll() {
                if let PlaybackEvent::Error(error) = event {
                    assert!(
                        error.contains("did not resolve")
                            || error.contains("Website not supported"),
                        "{error}"
                    );
                    if error.contains("Website not supported") {
                        return;
                    }
                }
            }
            assert!(Instant::now() < deadline, "no extraction failure reported");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "requires libmpv and NVPLAY_TEST_MEDIA pointing to a local video"]
    fn real_engine_loads_pauses_seeks_and_reports_errors() {
        let media = std::env::var("NVPLAY_TEST_MEDIA").expect("set NVPLAY_TEST_MEDIA");
        let mut player = Player::with_output("null", Some("null")).unwrap();
        player.load(&media).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut loaded = false;
        while Instant::now() < deadline {
            for event in player.poll() {
                match event {
                    PlaybackEvent::Loaded => loaded = true,
                    PlaybackEvent::Error(e) => panic!("{e}"),
                    _ => {}
                }
            }
            if loaded && player.state.duration > 0.0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(loaded && player.state.duration > 0.0, "video never loaded");
        player.command(&["set", "pause", "yes"]).unwrap();
        player.command(&["seek", "1", "absolute+exact"]).unwrap();
        player.command(&["set", "volume", "42"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            player.poll();
            if player.state.paused
                && (player.state.position - 1.0).abs() < 0.2
                && (player.state.volume - 42.0).abs() < 0.1
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(player.state.paused);
        assert!((player.state.position - 1.0).abs() < 0.2);
        assert!((player.state.volume - 42.0).abs() < 0.1);
        // Sparse keyframes must not turn a five-second jump into EOF.
        player.command(&["seek", "5", "relative+exact"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            player.poll();
            if (player.state.position - 6.0).abs() < 0.2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!((player.state.position - 6.0).abs() < 0.2);
        assert!(player.state.paused && !player.state.idle);
        player
            .command(&["set", "nvplay-intentionally-invalid-property", "yes"])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if player
                .poll()
                .iter()
                .any(|e| matches!(e, PlaybackEvent::Error(_)))
            {
                break;
            }
            assert!(Instant::now() < deadline, "missing command error");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
