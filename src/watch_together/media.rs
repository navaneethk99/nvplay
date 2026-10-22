//! One encoder feeds every peer. Only loopback RTP enters this module.
use super::Snapshot;
use std::{
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU8, AtomicU16, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, net::UdpSocket, process::Child, task::JoinHandle};
use webrtc::{
    rtp::packet::Packet,
    track::track_local::{TrackLocalWriter, track_local_static_rtp::TrackLocalStaticRTP},
    util::{Marshal, Unmarshal},
};

#[derive(Clone, Copy, Default)]
pub struct HlsOptions {
    extension_picky: bool,
    hardware_h264: bool,
}
impl HlsOptions {
    pub async fn discover(path: &Path) -> Self {
        let mut command = tokio::process::Command::from(crate::streaming::tool_command(path));
        command
            .kill_on_drop(true)
            .args(["-hide_banner", "-h", "demuxer=hls"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(5), command.output()).await;
        // Probe a real encode: listing an encoder does not mean this machine
        // can initialize its hardware session. Retain libx264 on other systems.
        let hardware_h264 = if cfg!(target_os = "macos")
            && std::env::var("NVPLAY_WATCH_SOFTWARE_ENCODER").as_deref() != Ok("1")
        {
            let mut probe = tokio::process::Command::from(crate::streaming::tool_command(path));
            probe
                .kill_on_drop(true)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=s=1920x1080:r=30",
                    "-frames:v",
                    "2",
                    "-c:v",
                    "h264_videotoolbox",
                    "-realtime",
                    "1",
                    "-allow_sw",
                    "0",
                    "-b:v",
                    "6000000",
                    "-profile:v",
                    "baseline",
                    "-f",
                    "null",
                    "-",
                ]);
            tokio::time::timeout(Duration::from_secs(5), probe.status())
                .await
                .ok()
                .and_then(Result::ok)
                .is_some_and(|status| status.success())
        } else {
            false
        };
        Self {
            hardware_h264,
            extension_picky: output.ok().and_then(Result::ok).is_some_and(|o| {
                o.status.success()
                    && String::from_utf8_lossy(&o.stdout).contains("-extension_picky")
            }),
        }
    }
    pub fn fallback_to_software(&mut self) -> bool {
        std::mem::replace(&mut self.hardware_h264, false)
    }
    fn apply(self, command: &mut tokio::process::Command, source: &str) {
        // Current FFmpeg rejects legitimate HLS segments whose URL suffix does
        // not match the container, including the provider's opaque segment URLs.
        // Keep this scoped to HLS inputs and retain the protocol whitelist.
        if self.extension_picky && is_hls(source) {
            command.args(["-extension_picky", "0"]);
        }
    }
}
fn is_hls(source: &str) -> bool {
    let path = url::Url::parse(source)
        .ok()
        .map(|url| url.path().to_owned())
        .unwrap_or_else(|| source.to_owned());
    Path::new(&path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("m3u8"))
}

pub struct Encoder {
    child: Child,
    video_encoder: Option<JoinHandle<Result<(), String>>>,
    rate: tokio::sync::watch::Sender<u64>,
    diagnostics: Option<JoinHandle<Vec<u8>>>,
    tasks: Vec<JoinHandle<()>>,
    pub preview: bool,
    preview_sent: Option<Arc<OnceLock<Instant>>>,
    drain_since: Option<Instant>,
    pub position: f64,
    pub speed: f64,
    pub width: i64,
    pub height: i64,
    pub bitrate: u64,
    pub started: Instant,
    aligned: bool,
    pub synchronizing_until: Option<Instant>,
}
impl Drop for Encoder {
    fn drop(&mut self) {
        if let Some(task) = &self.video_encoder {
            task.abort();
        }
        for task in &self.tasks {
            task.abort();
        }
        if let Some(task) = &self.diagnostics {
            task.abort();
        }
        let _ = self.child.start_kill();
    }
}
impl Encoder {
    pub fn align_playback(&mut self) -> Option<f64> {
        if self.preview || self.aligned {
            return None;
        }
        let first = *self.preview_sent.as_ref()?.get()?;
        self.aligned = true;
        self.started = first;
        Some(self.position + first.elapsed().as_secs_f64() * self.speed)
    }
    pub fn set_bitrate(&mut self, bitrate: u64) {
        self.bitrate = bitrate;
        self.rate.send_replace(bitrate);
    }
    pub async fn completed(&mut self) -> Result<bool, String> {
        if self
            .video_encoder
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            match self.video_encoder.take().unwrap().await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(encoder_error(&error)),
                Err(_) => return Err("The video encoder stopped unexpectedly".into()),
            }
        }
        match self.child.try_wait() {
            Ok(None) => Ok(false),
            Ok(Some(status)) => {
                // FFmpeg can exit before the paced UDP pump has forwarded the
                // final marker packet of a paused preview. Keep the pump alive.
                if status.success()
                    && self.preview
                    && self
                        .preview_sent
                        .as_ref()
                        .is_some_and(|sent| sent.get().is_none())
                {
                    let since = self.drain_since.get_or_insert_with(Instant::now);
                    if since.elapsed() < Duration::from_secs(2) {
                        return Ok(false);
                    }
                }
                let diagnostics = match self.diagnostics.take() {
                    Some(task) => task.await.unwrap_or_default(),
                    None => vec![],
                };
                if status.success() {
                    Ok(true)
                } else {
                    Err(encoder_error(&String::from_utf8_lossy(&diagnostics)))
                }
            }
            Err(error) => Err(format!("Could not check the sharing encoder: {error}")),
        }
    }
    pub async fn start(
        path: &Path,
        snapshot: &Snapshot,
        tracks: &Tracks,
        hls: HlsOptions,
    ) -> Result<Self, String> {
        let video = UdpSocket::bind("127.0.0.1:0")
            .await
            .map_err(|_| "Could not open the video encoder socket")?;
        let audio = UdpSocket::bind("127.0.0.1:0")
            .await
            .map_err(|_| "Could not open the audio encoder socket")?;
        let diagnostic = std::env::var("NVPLAY_WATCH_DIAGNOSTICS").as_deref() == Ok("1");
        if diagnostic {
            eprintln!(
                "encoder start position={:.3} bitrate={} preview={}",
                snapshot.state.position, snapshot.bitrate, snapshot.state.paused
            );
        }
        let mut command = tokio::process::Command::from(crate::streaming::tool_command(path));
        if diagnostic {
            command.args(["-progress", "pipe:2", "-stats_period", "1"]);
        }
        command
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(if snapshot.video {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stderr(std::process::Stdio::piped());
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-probesize",
            "262144",
            "-analyzeduration",
            "500000",
            "-ss",
            &snapshot.state.position.to_string(),
            "-protocol_whitelist",
            "file,http,https,tcp,tls,crypto,data",
        ]);
        network_input(&mut command, &snapshot.source);
        hls.apply(&mut command, &snapshot.source);
        if !snapshot.video_headers.is_empty() {
            command.args(["-headers", &snapshot.video_headers]);
        }
        command.args(["-i", &snapshot.source]);
        if let Some(audio) = &snapshot.audio_source {
            command.args([
                "-probesize",
                "262144",
                "-analyzeduration",
                "500000",
                "-ss",
                &snapshot.state.position.to_string(),
            ]);
            network_input(&mut command, audio);
            hls.apply(&mut command, audio);
            if !snapshot.audio_headers.is_empty() {
                command.args(["-headers", &snapshot.audio_headers]);
            }
            command.args(["-i", audio]);
        }
        let bitrate = if snapshot.bitrate == 0 {
            video_bitrate(snapshot.width, snapshot.height)
        } else {
            snapshot.bitrate
        };
        if snapshot.video {
            // Match the decoded video, including when a website resolver chooses
            // a different rendition. H.264 4:2:0 needs even dimensions; pad only
            // the exceptional odd edge instead of shrinking the picture.
            let size = if snapshot.width > 0 && snapshot.height > 0 {
                format!("scale={}:{},", snapshot.width, snapshot.height)
            } else {
                String::new()
            };
            let filter = format!(
                "{size}pad=ceil(iw/2)*2:ceil(ih/2)*2,setpts=(PTS-STARTPTS)/{},realtime",
                snapshot.state.speed
            );
            if snapshot.state.paused {
                command.args(["-frames:v", "1"]);
            }
            // An HLS master can list its smallest rendition first. Let FFmpeg
            // choose the highest-resolution video before matching desktop size,
            // rather than enlarging the lowest rendition to an HD frame.
            if !is_hls(&snapshot.source) {
                command.args(["-map", "0:v:0"]);
            }
            command.args([
                "-an",
                "-vf",
                &filter,
                "-c:v",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
                "-threads:v",
                "1",
                "-flush_packets",
                "1",
                "-f",
                "yuv4mpegpipe",
                "pipe:1",
            ]);
        }
        if snapshot.audio && !snapshot.state.paused {
            let filter = format!("{},arealtime", tempo_filter(snapshot.state.speed));
            command.args([
                "-map",
                if snapshot.audio_source.is_some() {
                    "1:a:0"
                } else {
                    "0:a:0"
                },
                "-vn",
                "-af",
                &filter,
                "-c:a",
                "libopus",
                "-ar",
                "48000",
                "-ac",
                "2",
                "-b:a",
                "96k",
                "-application",
                "lowdelay",
                "-f",
                "rtp",
                "-payload_type",
                "111",
                &format!(
                    "rtp://127.0.0.1:{}?pkt_size=1200",
                    audio.local_addr().unwrap().port()
                ),
            ]);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "Could not start FFmpeg for sharing")?;
        let (rate, rate_rx) = tokio::sync::watch::channel(bitrate);
        let video_encoder = child.stdout.take().map(|source| {
            super::video::start(
                path.to_owned(),
                source,
                hls.hardware_h264,
                rate_rx.clone(),
                video.local_addr().unwrap().port(),
            )
        });
        let mut stderr = child.stderr.take().expect("piped FFmpeg stderr");
        let diagnostics = tokio::spawn(async move {
            let mut tail = Vec::new();
            let mut buffer = [0; 1024];
            while let Ok(n) = stderr.read(&mut buffer).await {
                if n == 0 {
                    break;
                }
                if diagnostic {
                    for line in String::from_utf8_lossy(&buffer[..n]).lines() {
                        if line.starts_with("frame=")
                            || line.starts_with("out_time=")
                            || line.starts_with("speed=")
                        {
                            eprintln!("encoder {line}");
                        }
                    }
                }
                tail.extend_from_slice(&buffer[..n]);
                if tail.len() > 8192 {
                    tail.drain(..tail.len() - 8192);
                }
            }
            tail
        });
        let preview_sent = (snapshot.video).then(|| Arc::new(OnceLock::new()));
        let mut tasks = vec![];
        if snapshot.video {
            tasks.extend(pump(
                video,
                tracks.video.clone(),
                tracks.video_sequence.clone(),
                tracks.epoch,
                90_000,
                Some(rate_rx),
                preview_sent.clone(),
                Some(tracks.send_time_extension.clone()),
            ));
        }
        if snapshot.audio && !snapshot.state.paused {
            tasks.extend(pump(
                audio,
                tracks.audio.clone(),
                tracks.audio_sequence.clone(),
                tracks.epoch,
                48_000,
                None,
                None,
                None,
            ));
        }
        Ok(Self {
            child,
            video_encoder,
            rate,
            diagnostics: Some(diagnostics),
            tasks,
            preview: snapshot.state.paused,
            preview_sent,
            drain_since: None,
            position: snapshot.state.position,
            speed: snapshot.state.speed,
            width: snapshot.width,
            height: snapshot.height,
            bitrate,
            started: Instant::now(),
            aligned: false,
            synchronizing_until: None,
        })
    }
}
fn encoder_error(stderr: &str) -> String {
    if stderr.contains("Unknown encoder") && stderr.contains("libx264") {
        return "FFmpeg is missing the libx264 video encoder. Install an FFmpeg build with libx264.".into();
    }
    if stderr.contains("Unknown encoder") && stderr.contains("libopus") {
        return "FFmpeg is missing the libopus audio encoder. Install an FFmpeg build with libopus.".into();
    }
    // Signed stream URLs can appear in FFmpeg diagnostics. Keep the reason,
    // without displaying provider tokens or full media addresses in the UI.
    let detail = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(4)
        .map(|line| {
            line.split_whitespace()
                .map(|word| {
                    if word.contains("://") {
                        "[media address]"
                    } else {
                        word
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    if detail.is_empty() {
        "FFmpeg stopped while sharing the media without reporting a reason.".into()
    } else {
        format!(
            "FFmpeg could not share this media:\n{}",
            detail.chars().take(1000).collect::<String>()
        )
    }
}

fn network_input(command: &mut tokio::process::Command, source: &str) {
    if url::Url::parse(source).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
        command.args([
            "-rw_timeout",
            "5000000",
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_on_network_error",
            "1",
            "-reconnect_delay_max",
            "2",
        ]);
    }
}

fn tempo_filter(mut speed: f64) -> String {
    let mut parts = vec!["asetpts=PTS-STARTPTS".to_owned()];
    while speed > 2.0 {
        parts.push("atempo=2".into());
        speed /= 2.0;
    }
    while speed < 0.5 {
        parts.push("atempo=0.5".into());
        speed /= 0.5;
    }
    parts.push(format!("atempo={speed}"));
    parts.join(",")
}
pub struct Tracks {
    pub send_time_extension: Arc<AtomicU8>,
    pub video: Arc<TrackLocalStaticRTP>,
    pub audio: Arc<TrackLocalStaticRTP>,
    video_sequence: Arc<AtomicU16>,
    audio_sequence: Arc<AtomicU16>,
    epoch: Instant,
}
impl Tracks {
    pub fn new() -> Self {
        use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
        Self {
            video: Arc::new(TrackLocalStaticRTP::new(
                RTCRtpCodecCapability {
                    mime_type: "video/H264".into(),
                    clock_rate: 90_000,
                    sdp_fmtp_line:
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                            .into(),
                    ..Default::default()
                },
                "video".into(),
                "nvplay".into(),
            )),
            audio: Arc::new(TrackLocalStaticRTP::new(
                RTCRtpCodecCapability {
                    mime_type: "audio/opus".into(),
                    clock_rate: 48_000,
                    channels: 2,
                    ..Default::default()
                },
                "audio".into(),
                "nvplay".into(),
            )),
            send_time_extension: Arc::new(AtomicU8::new(0)),
            video_sequence: Arc::new(AtomicU16::new(0)),
            audio_sequence: Arc::new(AtomicU16::new(0)),
            epoch: Instant::now(),
        }
    }
}
// Bound bursts without changing resolution. The old unconstrained CRF output
// could exceed 30 Mbps for a single 1080p viewer and congest every receiver.
pub(super) fn video_bitrate(width: i64, height: i64) -> u64 {
    let pixels = (width.max(1) as u64).saturating_mul(height.max(1) as u64);
    (pixels.saturating_mul(3)).clamp(2_000_000, 20_000_000)
}
/// Adapt the single desktop-to-server uplink. Viewer loss is handled at the relay.
/// Resolution stays fixed. Hysteresis avoids constant encoder restarts.
pub(super) struct AdaptiveRate {
    pub rate: u64,
    minimum: u64,
    changed: Instant,
    congested: Instant,
}
impl AdaptiveRate {
    pub fn new(ceiling: u64, now: Instant) -> Self {
        Self {
            rate: ceiling.min(6_000_000),
            minimum: (ceiling / 3).clamp(750_000, 8_000_000),
            changed: now - Duration::from_secs(5),
            congested: now,
        }
    }
    pub fn feedback(&mut self, loss: u8, estimate: u64, ceiling: u64, now: Instant) -> bool {
        if loss >= 13 {
            self.congested = now;
        }
        let mut target = self.rate;
        if now.duration_since(self.changed) >= Duration::from_secs(3) {
            if loss >= 51 {
                target = self.rate / 2;
            } else if loss >= 13 {
                target = self.rate * 4 / 5;
            }
            if estimate > 0 && estimate < target {
                target = estimate * 4 / 5;
            }
        }
        if now.duration_since(self.congested) >= Duration::from_secs(20)
            && now.duration_since(self.changed) >= Duration::from_secs(10)
        {
            target = target.saturating_mul(5) / 4;
        }
        let floor = self.minimum;
        target = target.clamp(floor, ceiling.max(floor));
        if target == self.rate {
            return false;
        }
        self.rate = target;
        self.changed = now;
        true
    }
}

struct Pacer {
    bits_per_second: Option<u64>,
    deadline: Instant,
}
impl Pacer {
    fn new(bitrate: Option<u64>) -> Self {
        // Leave room for RTP headers and VBV fluctuations. Small packet batches
        // avoid a timer per datagram while limiting keyframe microbursts.
        Self {
            bits_per_second: bitrate.map(|b| b * 12 / 10),
            deadline: Instant::now(),
        }
    }
    fn packet(&mut self, bytes: usize, now: Instant) -> Option<Instant> {
        let bitrate = self.bits_per_second?;
        self.deadline = self.deadline.max(now - Duration::from_millis(2))
            + Duration::from_secs_f64(bytes as f64 * 8.0 / bitrate as f64);
        (self.deadline.duration_since(now) >= Duration::from_millis(2)).then_some(self.deadline)
    }
}
// FFmpeg picks fresh random RTP sequence numbers/timestamps on each seek. Rewrite
// them onto a continuous session clock, or browser jitter buffers discard the seek.
// Keep incoming gaps visible to the receiver. Closing sequence gaps hides
// missing H.264 fragments from loss detection and can produce corrupt frames.
#[derive(Default)]
struct Sequence {
    previous: Option<u16>,
}
impl Sequence {
    fn map(&mut self, input: u16, next: &AtomicU16) -> Option<u16> {
        let step = self
            .previous
            .map_or(1, |previous| input.wrapping_sub(previous));
        if step == 0 || step >= 32768 {
            return None;
        }
        self.previous = Some(input);
        Some(
            next.fetch_add(step, Ordering::Relaxed)
                .wrapping_add(step - 1),
        )
    }
}
fn pump(
    socket: UdpSocket,
    track: Arc<TrackLocalStaticRTP>,
    sequence: Arc<AtomicU16>,
    epoch: Instant,
    rate: u32,
    bitrate: Option<tokio::sync::watch::Receiver<u64>>,
    preview_sent: Option<Arc<OnceLock<Instant>>>,
    send_time_extension: Option<Arc<AtomicU8>>,
) -> Vec<JoinHandle<()>> {
    // Receiving must never wait for network pacing or a slow peer. A bounded
    // queue absorbs keyframe bursts; overflow is observable as RTP packet loss.
    let capacity = (bitrate
        .as_ref()
        .map(|rate| *rate.borrow())
        .unwrap_or(128_000)
        / 8
        / 1200)
        .clamp(256, 4096) as usize;
    let (tx, mut rx) = tokio::sync::mpsc::channel(capacity);
    let ingest = tokio::spawn(async move {
        let mut buffer = [0_u8; 2048];
        while let Ok((n, _)) = socket.recv_from(&mut buffer).await {
            let Ok(packet) = Packet::unmarshal(&mut &buffer[..n]) else {
                continue;
            };
            match tx.try_send((packet, n)) {
                Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
            }
        }
    });
    let forward = tokio::spawn(async move {
        let mut origin = None;
        let mut mapper = Sequence::default();
        let mut pacer = Pacer::new(bitrate.as_ref().map(|rate| *rate.borrow()));
        let mut ssrc = None;
        while let Some((mut packet, n)) = rx.recv().await {
            if ssrc != Some(packet.header.ssrc) {
                ssrc = Some(packet.header.ssrc);
                origin = None;
                mapper = Sequence::default();
            }
            if let Some(rate) = &bitrate {
                pacer.bits_per_second = Some(rate.borrow().saturating_mul(12) / 10);
            }
            let Some(mapped) = mapper.map(packet.header.sequence_number, &sequence) else {
                continue;
            };
            let (input, output) = *origin.get_or_insert_with(|| {
                (
                    packet.header.timestamp,
                    (epoch.elapsed().as_micros() * u128::from(rate) / 1_000_000) as u32,
                )
            });
            packet.header.timestamp =
                output.wrapping_add(packet.header.timestamp.wrapping_sub(input));
            packet.header.sequence_number = mapped;
            if let Some(deadline) = pacer.packet(n, Instant::now()) {
                tokio::time::sleep_until(deadline.into()).await;
            }
            if let Some(id) = send_time_extension
                .as_ref()
                .map(|id| id.load(Ordering::Relaxed))
                .filter(|id| *id > 0)
                && let Ok(extension) =
                    webrtc::rtp::extension::abs_send_time_extension::AbsSendTimeExtension::new(
                        std::time::SystemTime::now(),
                    )
                    .marshal()
            {
                let _ = packet.header.set_extension(id, extension);
            }
            let sent = track.write_rtp(&packet).await.is_ok();
            if sent
                && packet.header.marker
                && let Some(done) = &preview_sent
            {
                done.get_or_init(Instant::now);
            }
        }
    });
    vec![ingest, forward]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires FFmpeg; verifies bitrate changes continue after the original source is removed"]
    async fn bitrate_changes_do_not_reopen_the_source() {
        let fixture = crate::subtitles::library::tests::Fixture::new();
        let file = fixture.0.join("persistent-source.mp4");
        let ffmpeg = crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg").unwrap();
        assert!(
            std::process::Command::new(&ffmpeg)
                .args([
                    "-nostdin",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=320x180:rate=25",
                    "-t",
                    "12",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast"
                ])
                .arg(&file)
                .status()
                .unwrap()
                .success()
        );
        let snapshot = Snapshot {
            source: file.to_string_lossy().into_owned(),
            audio_source: None,
            video_headers: String::new(),
            audio_headers: String::new(),
            video: true,
            audio: false,
            width: 320,
            height: 180,
            bitrate: 2_000_000,
            access: Default::default(),
            state: watch_protocol::Playback {
                position: 0.0,
                duration: 12.0,
                paused: false,
                buffering: false,
                speed: 1.0,
                title: "Continuous decoding".into(),
            },
        };
        let tracks = Tracks::new();
        let mut encoder = Encoder::start(&ffmpeg, &snapshot, &tracks, HlsOptions::default())
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while encoder.preview_sent.as_ref().unwrap().get().is_none() {
            assert!(Instant::now() < deadline, "missing initial frame");
            assert!(!encoder.completed().await.unwrap());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::fs::remove_file(&file).unwrap();
        for bitrate in [500_000, 1_500_000, 750_000] {
            let before = tracks.video_sequence.load(Ordering::Relaxed);
            encoder.set_bitrate(bitrate);
            let deadline = Instant::now() + Duration::from_secs(2);
            while tracks
                .video_sequence
                .load(Ordering::Relaxed)
                .wrapping_sub(before)
                < 20
            {
                assert!(
                    Instant::now() < deadline,
                    "bitrate change stalled the stream"
                );
                assert!(!encoder.completed().await.unwrap());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg; checks first-frame delivery after a deep seek in a long GOP"]
    async fn seeking_paces_decoded_frames_instead_of_preroll() {
        let fixture = crate::subtitles::library::tests::Fixture::new();
        let file = fixture.0.join("long-gop.mp4");
        let ffmpeg = crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg").unwrap();
        assert!(
            std::process::Command::new(&ffmpeg)
                .args([
                    "-nostdin",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=320x180:rate=25",
                    "-t",
                    "9",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-g",
                    "250",
                    "-keyint_min",
                    "250",
                    "-sc_threshold",
                    "0",
                ])
                .arg(&file)
                .status()
                .unwrap()
                .success()
        );
        let snapshot = Snapshot {
            source: file.to_string_lossy().into_owned(),
            audio_source: None,
            video_headers: String::new(),
            audio_headers: String::new(),
            video: true,
            audio: false,
            width: 320,
            height: 180,
            bitrate: 2_000_000,
            access: Default::default(),
            state: watch_protocol::Playback {
                position: 7.0,
                duration: 9.0,
                paused: false,
                buffering: false,
                speed: 1.0,
                title: "Long GOP".into(),
            },
        };
        let started = Instant::now();
        let mut encoder = Encoder::start(&ffmpeg, &snapshot, &Tracks::new(), HlsOptions::default())
            .await
            .unwrap();
        while encoder.preview_sent.as_ref().unwrap().get().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "deep seeking delayed the first frame"
            );
            assert!(!encoder.completed().await.unwrap());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn sequence_mapping_preserves_loss_wrap_and_encoder_restart() {
        let next = AtomicU16::new(65534);
        let mut map = Sequence::default();
        assert_eq!(map.map(65534, &next), Some(65534));
        assert_eq!(map.map(1, &next), Some(1)); // missing 65535 and 0 remain gaps
        assert_eq!(map.map(1, &next), None); // duplicates do not consume numbers
        assert_eq!(map.map(0, &next), None); // late datagrams do not rewind
        assert_eq!(map.map(2, &next), Some(2));
        let mut restarted = Sequence::default();
        assert_eq!(restarted.map(23000, &next), Some(3));
    }
    #[test]
    fn congestion_keeps_a_resolution_based_quality_floor_and_recovers() {
        let at = Instant::now();
        let ceiling = 6_220_800;
        let mut rate = AdaptiveRate::new(ceiling, at);
        assert_eq!(rate.rate, 6_000_000);
        assert!(rate.feedback(80, 0, ceiling, at));
        assert_eq!(rate.rate, 3_000_000);
        assert!(rate.feedback(80, 0, ceiling, at + Duration::from_secs(4)));
        assert_eq!(rate.rate, ceiling / 3);
        assert!(!rate.feedback(0, 0, ceiling, at + Duration::from_secs(10)));
        assert!(rate.feedback(0, 0, ceiling, at + Duration::from_secs(25)));
        assert_eq!(rate.rate, ceiling / 3 * 5 / 4);
        assert!(rate.feedback(0, 500_000, ceiling, at + Duration::from_secs(29)));
        assert_eq!(rate.rate, ceiling / 3);
    }

    #[test]
    fn pacing_bounds_bursts_without_slow_timer_drift() {
        let mut pacer = Pacer::new(Some(6_000_000));
        let at = pacer.deadline;
        assert!(pacer.packet(1200, at).is_none());
        let deadline = pacer.packet(1200, at).unwrap();
        assert!(deadline - at < Duration::from_millis(3));
        // A late timer can catch up, rather than permanently reducing throughput.
        let late = deadline + Duration::from_millis(1);
        assert!(pacer.packet(1200, late).is_none());
        let idle = late + Duration::from_secs(5);
        assert!(pacer.packet(1200, idle).is_none());
        assert!(pacer.deadline >= idle - Duration::from_millis(2));
        assert!(Pacer::new(None).packet(1200, idle).is_none());
        assert_eq!(video_bitrate(1920, 1080), 6_220_800);
        assert_eq!(video_bitrate(i64::MAX, i64::MAX), 20_000_000);
    }

    #[test]
    fn playback_rates_fit_ffmpeg_tempo_limits() {
        assert_eq!(tempo_filter(4.0), "asetpts=PTS-STARTPTS,atempo=2,atempo=2");
        assert_eq!(
            tempo_filter(0.25),
            "asetpts=PTS-STARTPTS,atempo=0.5,atempo=0.5"
        );
    }
    #[test]
    fn encoder_diagnostics_explain_the_error_without_signed_urls() {
        let message = encoder_error(
            "[hls] HTTP error 403 Forbidden\nError opening input file https://media.example/movie?token=private-secret\n",
        );
        assert!(message.contains("403 Forbidden"));
        assert!(!message.contains("private-secret"));
        assert!(!message.contains("media.example"));
        assert!(encoder_error("Unknown encoder 'libopus'").contains("missing the libopus"));
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg with libx264/libopus; creates HTTP HLS with nonstandard segment URLs"]
    async fn real_hls_encoder_accepts_opaque_segment_urls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let ffmpeg = crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg").expect("FFmpeg");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder =
            std::env::temp_dir().join(format!("nvplay-hls-sharing-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _fixture = Fixture(folder.clone());
        let status = tokio::process::Command::from(crate::streaming::tool_command(&ffmpeg))
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "8",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "30",
                "-c:a",
                "aac",
                "-hls_time",
                "1",
                "-hls_playlist_type",
                "vod",
                "-hls_segment_filename",
                folder.join("segment-%02d.opaque").to_str().unwrap(),
                "-f",
                "hls",
                folder.join("index.m3u8").to_str().unwrap(),
            ])
            .status()
            .await
            .unwrap();
        assert!(status.success());
        let files: std::collections::HashMap<String, Vec<u8>> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| {
                let p = entry.unwrap().path();
                (
                    format!("/{}", p.file_name().unwrap().to_str().unwrap()),
                    std::fs::read(p).unwrap(),
                )
            })
            .collect();
        let files = Arc::new(files);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = format!("http://{}/index.m3u8", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let files = files.clone();
                tokio::spawn(async move {
                    let mut request = vec![0; 8192];
                    let Ok(n) = socket.read(&mut request).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&request[..n]);
                    let name = request.split_whitespace().nth(1).unwrap_or("");
                    let Some(bytes) = files.get(name) else {
                        return;
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(bytes).await;
                });
            }
        });
        let snapshot = Snapshot {
            source,
            audio_source: None,
            video_headers: String::new(),
            audio_headers: String::new(),
            video: true,
            audio: true,
            width: 0,
            height: 0,
            bitrate: 0,
            access: Default::default(),
            state: watch_protocol::Playback {
                position: 0.0,
                duration: 8.0,
                paused: false,
                buffering: false,
                speed: 1.0,
                title: "HLS regression".into(),
            },
        };
        let tracks = Tracks::new();
        let hls = HlsOptions::discover(&ffmpeg).await;
        let mut encoder = Encoder::start(&ffmpeg, &snapshot, &tracks, hls)
            .await
            .unwrap();
        let start = Instant::now();
        let mut failure = None;
        while start.elapsed() < std::time::Duration::from_secs(12) {
            if let Err(error) = encoder.completed().await {
                failure = Some(error);
                break;
            }
            if tracks.video_sequence.load(Ordering::Relaxed) > 10
                && tracks.audio_sequence.load(Ordering::Relaxed) > 3
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let video = tracks.video_sequence.load(Ordering::Relaxed);
        let audio = tracks.audio_sequence.load(Ordering::Relaxed);
        drop(encoder);
        server.abort();
        assert!(failure.is_none(), "{}", failure.unwrap_or_default());
        assert!(
            video > 10 && audio > 3,
            "HLS did not produce both RTP streams: video={video}, audio={audio}"
        );
    }
}
