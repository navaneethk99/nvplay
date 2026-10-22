//! One encoder feeds every peer. Only loopback RTP enters this module.
use super::Snapshot;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
    time::Instant,
};
use tokio::{net::UdpSocket, process::Child, task::JoinHandle};
use webrtc::{
    rtp::packet::Packet,
    track::track_local::{TrackLocalWriter, track_local_static_rtp::TrackLocalStaticRTP},
    util::Unmarshal,
};

pub struct Encoder {
    child: Child,
    tasks: Vec<JoinHandle<()>>,
    pub preview: bool,
    pub position: f64,
    pub speed: f64,
    pub started: Instant,
}
impl Drop for Encoder {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let _ = self.child.start_kill();
    }
}
impl Encoder {
    pub fn completed(&mut self) -> Result<bool, String> {
        match self.child.try_wait() {
            Ok(None) => Ok(false),
            Ok(Some(status)) if status.success() => Ok(true),
            _ => Err("FFmpeg could not stream this media. Sharing needs libx264 and libopus, and a source FFmpeg can open.".into()),
        }
    }
    pub async fn start(path: &Path, snapshot: &Snapshot, tracks: &Tracks) -> Result<Self, String> {
        let video = UdpSocket::bind("127.0.0.1:0")
            .await
            .map_err(|_| "Could not open the video encoder socket")?;
        let audio = UdpSocket::bind("127.0.0.1:0")
            .await
            .map_err(|_| "Could not open the audio encoder socket")?;
        let mut command = tokio::process::Command::from(crate::streaming::tool_command(path));
        command
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-readrate",
            &snapshot.state.speed.to_string(),
            "-ss",
            &snapshot.state.position.to_string(),
            "-protocol_whitelist",
            "file,http,https,tcp,tls,crypto,data",
            "-i",
            &snapshot.source,
        ]);
        if let Some(audio) = &snapshot.audio_source {
            command.args([
                "-readrate",
                &snapshot.state.speed.to_string(),
                "-ss",
                &snapshot.state.position.to_string(),
                "-i",
                audio,
            ]);
        }
        if snapshot.video {
            let filter = format!(
                "scale=w='min(1280,iw)':h=-2,setsar=1,setpts=(PTS-STARTPTS)/{},fps=30",
                snapshot.state.speed
            );
            if snapshot.state.paused {
                command.args(["-frames:v", "1"]);
            }
            command.args([
                "-map",
                "0:v:0",
                "-an",
                "-vf",
                &filter,
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-profile:v",
                "baseline",
                "-pix_fmt",
                "yuv420p",
                "-b:v",
                "2000k",
                "-maxrate",
                "2200k",
                "-bufsize",
                "1000k",
                "-g",
                "30",
                "-keyint_min",
                "30",
                "-sc_threshold",
                "0",
                "-f",
                "rtp",
                "-payload_type",
                "102",
                &format!(
                    "rtp://127.0.0.1:{}?pkt_size=1200",
                    video.local_addr().unwrap().port()
                ),
            ]);
        }
        if snapshot.audio && !snapshot.state.paused {
            let filter = tempo_filter(snapshot.state.speed);
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
        let child = command
            .spawn()
            .map_err(|_| "Could not start FFmpeg for sharing")?;
        let mut tasks = vec![];
        if snapshot.video {
            tasks.push(pump(
                video,
                tracks.video.clone(),
                tracks.video_sequence.clone(),
                tracks.epoch,
                90_000,
            ));
        }
        if snapshot.audio && !snapshot.state.paused {
            tasks.push(pump(
                audio,
                tracks.audio.clone(),
                tracks.audio_sequence.clone(),
                tracks.epoch,
                48_000,
            ));
        }
        Ok(Self {
            child,
            tasks,
            preview: snapshot.state.paused,
            position: snapshot.state.position,
            speed: snapshot.state.speed,
            started: Instant::now(),
        })
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
            video_sequence: Arc::new(AtomicU16::new(0)),
            audio_sequence: Arc::new(AtomicU16::new(0)),
            epoch: Instant::now(),
        }
    }
}
// FFmpeg picks fresh random RTP sequence numbers/timestamps on each seek. Rewrite
// them onto a continuous session clock, or browser jitter buffers discard the seek.
fn pump(
    socket: UdpSocket,
    track: Arc<TrackLocalStaticRTP>,
    sequence: Arc<AtomicU16>,
    epoch: Instant,
    rate: u32,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0_u8; 2048];
        let mut origin = None;
        while let Ok((n, _)) = socket.recv_from(&mut buffer).await {
            let Ok(mut packet) = Packet::unmarshal(&mut &buffer[..n]) else {
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
            packet.header.sequence_number = sequence.fetch_add(1, Ordering::Relaxed);
            let _ = track.write_rtp(&packet).await;
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playback_rates_fit_ffmpeg_tempo_limits() {
        assert_eq!(tempo_filter(4.0), "asetpts=PTS-STARTPTS,atempo=2,atempo=2");
        assert_eq!(
            tempo_filter(0.25),
            "asetpts=PTS-STARTPTS,atempo=0.5,atempo=0.5"
        );
    }
}
