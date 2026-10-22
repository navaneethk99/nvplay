//! Encrypted WebRTC fan-out: one desktop uplink, independent viewer transports.
//! Media is forwarded without decoding, resizing, or transcoding.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{OnceCell, broadcast};
use watch_protocol::Message;
use webrtc::{
    api::{
        API, APIBuilder, interceptor_registry::register_default_interceptors,
        media_engine::MediaEngine, setting_engine::SettingEngine,
    },
    ice::{
        udp_mux::{UDPMuxDefault, UDPMuxParams},
        udp_network::UDPNetwork,
    },
    ice_transport::{ice_candidate::RTCIceCandidateInit, ice_candidate_type::RTCIceCandidateType},
    peer_connection::{
        RTCPeerConnection, configuration::RTCConfiguration,
        sdp::session_description::RTCSessionDescription,
    },
    rtp::packet::Packet,
    rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTPCodecType},
    track::track_local::{
        TrackLocal, TrackLocalWriter, track_local_static_rtp::TrackLocalStaticRTP,
    },
};
const PUBLISHER: &str = "__publisher";
static API_INSTANCE: OnceCell<API> = OnceCell::const_new();
async fn api() -> Result<&'static API, String> {
    API_INSTANCE
        .get_or_try_init(|| async {
            let mut engine = MediaEngine::default();
            engine
                .register_default_codecs()
                .map_err(|_| "Could not register media codecs")?;
            let registry = register_default_interceptors(Default::default(), &mut engine)
                .map_err(|_| "Could not initialize media feedback")?;
            engine
                .register_header_extension(
                    webrtc::rtp_transceiver::rtp_codec::RTCRtpHeaderExtensionCapability {
                        uri: webrtc::sdp::extmap::ABS_SEND_TIME_URI.into(),
                    },
                    RTPCodecType::Video,
                    None,
                )
                .map_err(|_| "Could not configure network timing")?;
            let mut settings = SettingEngine::default();
            if let Ok(ip) = std::env::var("NVPLAY_SFU_PUBLIC_IP") {
                ip.parse::<std::net::Ipv4Addr>()
                    .map_err(|_| "Invalid SFU public IP")?;
                let socket = tokio::net::UdpSocket::bind("0.0.0.0:50000")
                    .await
                    .map_err(|_| "Could not bind media port")?;
                settings.set_udp_network(UDPNetwork::Muxed(UDPMuxDefault::new(UDPMuxParams::new(
                    socket,
                ))));
                settings.set_nat_1to1_ips(vec![ip], RTCIceCandidateType::Host);
            }
            Ok(APIBuilder::new()
                .with_media_engine(engine)
                .with_interceptor_registry(registry)
                .with_setting_engine(settings)
                .build())
        })
        .await
}

pub struct Connection {
    pc: Arc<RTCPeerConnection>,
    pending: Vec<RTCIceCandidateInit>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Connection {
    async fn new(id: &str, tx: super::Tx) -> Result<Self, String> {
        let pc = Arc::new(
            api()
                .await?
                .new_peer_connection(RTCConfiguration::default())
                .await
                .map_err(|_| "Could not create media transport")?,
        );
        let id = id.to_owned();
        if id == PUBLISHER {
            let errors = tx.clone();
            pc.on_peer_connection_state_change(Box::new(move |state| {
                let tx = errors.clone();
                Box::pin(async move {
                    if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed {
                        let _ = tx.send(Message::Error { message: "The streaming uplink disconnected. Start a new watch party to reconnect.".into() }).await;
                    }
                })
            }));
        }
        pc.on_ice_candidate(Box::new(move |candidate| {
            let tx = tx.clone();
            let peer = id.clone();
            Box::pin(async move {
                if let Some(candidate) = candidate
                    .and_then(|c| c.to_json().ok())
                    .and_then(|c| serde_json::to_value(c).ok())
                {
                    let _ = tx.send(Message::Candidate { peer, candidate }).await;
                }
            })
        }));
        Ok(Self {
            pc,
            pending: vec![],
            tasks: vec![],
        })
    }
    pub async fn offer(&mut self, sdp: String, peer: &str, tx: super::Tx) -> Result<(), String> {
        self.pc
            .set_remote_description(
                RTCSessionDescription::offer(sdp).map_err(|_| "Invalid viewer offer")?,
            )
            .await
            .map_err(|_| "Could not accept viewer offer")?;
        let answer = self
            .pc
            .create_answer(None)
            .await
            .map_err(|_| "Could not answer viewer")?;
        let sdp = answer.sdp.clone();
        self.pc
            .set_local_description(answer)
            .await
            .map_err(|_| "Could not start viewer transport")?;
        self.flush().await;
        tx.send(Message::Answer {
            peer: peer.into(),
            sdp,
        })
        .await
        .map_err(|_| "Viewer disconnected")?;
        Ok(())
    }
    pub async fn answer(&mut self, sdp: String) -> Result<(), String> {
        self.pc
            .set_remote_description(
                RTCSessionDescription::answer(sdp).map_err(|_| "Invalid media answer")?,
            )
            .await
            .map_err(|_| "Could not accept media answer")?;
        self.flush().await;
        Ok(())
    }
    async fn flush(&mut self) {
        for candidate in self.pending.drain(..) {
            let _ = self.pc.add_ice_candidate(candidate).await;
        }
    }
    pub async fn candidate(&mut self, value: serde_json::Value) {
        if let Ok(candidate) = serde_json::from_value(value) {
            if self.pc.remote_description().await.is_some() {
                let _ = self.pc.add_ice_candidate(candidate).await;
            } else if self.pending.len() < 64 {
                self.pending.push(candidate);
            }
        }
    }
    pub async fn close(mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        let _ = self.pc.close().await;
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let pc = self.pc.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = pc.close().await;
            });
        }
    }
}

/// Compare actual packet send time with arrival progress. Source fetch/decoder
/// stalls must not be mistaken for network queue growth. Clocks need no shared
/// epoch: only relative progress is compared.
struct UplinkMonitor {
    epoch: Instant,
    sample: Instant,
    first_timestamp: u32,
    ticks: u64,
    baseline_ms: i128,
    bytes: u64,
}
impl UplinkMonitor {
    fn new(timestamp: u32, now: Instant) -> Self {
        Self {
            epoch: now,
            sample: now,
            first_timestamp: timestamp,
            ticks: 0,
            baseline_ms: 0,
            bytes: 0,
        }
    }
    fn observe(
        &mut self,
        timestamp: u32,
        bytes: usize,
        marker: bool,
        now: Instant,
    ) -> Option<Message> {
        // Absolute-send-time is a 24-bit, 6.18 fixed-point clock. Resolve its
        // 64-second wraps against elapsed arrival time, including long pauses.
        const PERIOD: i128 = 1 << 24;
        const CLOCK: i128 = 1 << 18;
        let elapsed = now.duration_since(self.epoch);
        let expected = (elapsed.as_secs_f64() * CLOCK as f64) as i128;
        let raw = i128::from(timestamp.wrapping_sub(self.first_timestamp) & 0x00ff_ffff);
        let wraps = (expected - raw + PERIOD / 2).div_euclid(PERIOD);
        let sent = raw + wraps * PERIOD;
        if sent < i128::from(self.ticks) {
            return None;
        }
        self.ticks = sent as u64;
        self.bytes += bytes as u64;
        let transit_ms = elapsed.as_millis() as i128 - sent * 1000 / CLOCK;
        self.baseline_ms = self.baseline_ms.min(transit_ms);
        let span = now.duration_since(self.sample);
        if !marker || span < Duration::from_secs(1) {
            return None;
        }
        let bitrate = (self.bytes as f64 * 8.0 / span.as_secs_f64()) as u64;
        self.sample = now;
        self.bytes = 0;
        Some(Message::Uplink {
            bitrate,
            queue_ms: (transit_ms - self.baseline_ms).max(0) as u64,
        })
    }
}

pub struct Relay {
    pub publisher: Connection,
    video: broadcast::Sender<Packet>,
    audio: broadcast::Sender<Packet>,
    has_video: bool,
    has_audio: bool,
}
impl Relay {
    pub async fn publish(
        sdp: String,
        tx: super::Tx,
        uplink_feedback: bool,
    ) -> Result<Self, String> {
        let mut publisher = Connection::new(PUBLISHER, tx.clone()).await?;
        let (video, _) = broadcast::channel(1024);
        let (audio, _) = broadcast::channel(256);
        let v = video.clone();
        let a = audio.clone();
        let feedback = tx.clone();
        publisher.pc.on_track(Box::new(move |track, receiver, _| {
            let is_video = track.kind() == RTPCodecType::Video;
            let feedback = feedback.clone();
            let output = if is_video { v.clone() } else { a.clone() };
            Box::pin(async move {
                let send_time_id = receiver
                    .get_parameters()
                    .await
                    .header_extensions
                    .iter()
                    .find(|e| e.uri == webrtc::sdp::extmap::ABS_SEND_TIME_URI)
                    .map(|e| e.id as u8);
                // This reader never waits for a slow viewer's network writer.
                tokio::spawn(async move {
                    let mut monitor = None;
                    while let Ok((mut packet, _)) = track.read_rtp().await {
                        if is_video
                            && uplink_feedback
                            && let Some(extension) =
                                send_time_id.and_then(|id| packet.header.get_extension(id))
                            && extension.len() == 3
                        {
                            let send_time = (u32::from(extension[0]) << 16)
                                | (u32::from(extension[1]) << 8)
                                | u32::from(extension[2]);
                            let now = Instant::now();
                            let monitor =
                                monitor.get_or_insert_with(|| UplinkMonitor::new(send_time, now));
                            if let Some(report) = monitor.observe(
                                send_time,
                                packet.payload.len() + 12,
                                packet.header.marker,
                                now,
                            ) {
                                let _ = feedback.try_send(report);
                            }
                        }
                        // Publisher extension IDs differ from viewer negotiation.
                        packet.header.extensions.clear();
                        packet.header.extension = false;
                        let _ = output.send(packet);
                    }
                });
            })
        }));
        let has_video = sdp.contains("m=video ");
        let has_audio = sdp.contains("m=audio ");
        publisher
            .pc
            .set_remote_description(
                RTCSessionDescription::offer(sdp).map_err(|_| "Invalid publisher offer")?,
            )
            .await
            .map_err(|_| "Could not accept publisher")?;
        let answer = publisher
            .pc
            .create_answer(None)
            .await
            .map_err(|_| "Could not answer publisher")?;
        let sdp = answer.sdp.clone();
        publisher
            .pc
            .set_local_description(answer)
            .await
            .map_err(|_| "Could not start publisher transport")?;
        publisher.flush().await;
        tx.send(Message::Answer {
            peer: PUBLISHER.into(),
            sdp,
        })
        .await
        .map_err(|_| "Publisher disconnected")?;
        Ok(Self {
            publisher,
            video,
            audio,
            has_video,
            has_audio,
        })
    }
    pub async fn subscribe(
        &self,
        id: &str,
        tx: super::Tx,
        host: super::Tx,
        client_offers: bool,
    ) -> Result<Connection, String> {
        let mut connection = Connection::new(id, tx.clone()).await?;
        let peer_id = id.to_owned();
        let notified = Arc::new(std::sync::atomic::AtomicBool::new(false));
        connection.pc.on_peer_connection_state_change(Box::new(move |state| {
            let tx = host.clone(); let peer = peer_id.clone(); let notified = notified.clone();
            Box::pin(async move {
                if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected
                    && !notified.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    // Trigger a fresh still frame only once this viewer can receive it.
                    let _ = tx.send(Message::PeerJoined { peer }).await;
                }
            })
        }));
        for (is_video, enabled, source) in [
            (true, self.has_video, &self.video),
            (false, self.has_audio, &self.audio),
        ] {
            if !enabled {
                continue;
            }
            let codec = if is_video {
                RTCRtpCodecCapability {
                    mime_type: "video/H264".into(),
                    clock_rate: 90000,
                    sdp_fmtp_line:
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                            .into(),
                    ..Default::default()
                }
            } else {
                RTCRtpCodecCapability {
                    mime_type: "audio/opus".into(),
                    clock_rate: 48000,
                    channels: 2,
                    ..Default::default()
                }
            };
            let local = Arc::new(TrackLocalStaticRTP::new(
                codec,
                if is_video { "video" } else { "audio" }.into(),
                "nvplay".into(),
            ));
            let sender = connection
                .pc
                .add_track(local.clone() as Arc<dyn TrackLocal + Send + Sync>)
                .await
                .map_err(|_| "Could not attach forwarded media")?;
            // Default NACK interceptors retransmit from this viewer's own cache,
            // independently of other viewers and without a round trip to the host.
            connection.tasks.push(tokio::spawn(async move {
                while sender.read_rtcp().await.is_ok() {}
            }));
            let mut receiver = source.subscribe();
            let transport = connection.pc.clone();
            connection.tasks.push(tokio::spawn(async move {
                loop {
                    match receiver.recv().await {
                        Ok(packet) => {
                            // Never replay a negotiation-time backlog at line rate:
                            // that burst can overwhelm TURN/mobile receive buffers.
                            if transport.connection_state() != webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected { continue; }
                            if local.write_rtp(&packet).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Prefer current live media to accumulating delay.
                            while receiver.try_recv().is_ok() {}
                            continue;
                        },
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }));
        }
        if client_offers {
            return Ok(connection);
        }
        let offer = connection
            .pc
            .create_offer(None)
            .await
            .map_err(|_| "Could not offer forwarded media")?;
        let sdp = offer.sdp.clone();
        connection
            .pc
            .set_local_description(offer)
            .await
            .map_err(|_| "Could not start viewer transport")?;
        tx.send(Message::Offer {
            peer: id.into(),
            sdp,
        })
        .await
        .map_err(|_| "Viewer disconnected")?;
        Ok(connection)
    }
    pub async fn close(self) {
        self.publisher.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uplink_detects_delay_without_loss_and_handles_wrap_pause_and_reordering() {
        let start = Instant::now();
        let timestamp = (1 << 24) - (1 << 17);
        let mut monitor = UplinkMonitor::new(timestamp, start);
        let stamp = |ticks: u32| timestamp.wrapping_add(ticks) & 0x00ff_ffff;
        let Some(Message::Uplink { queue_ms, .. }) = monitor.observe(
            stamp(1 << 18),
            1000,
            true,
            start + Duration::from_millis(1300),
        ) else {
            panic!("missing feedback");
        };
        assert_eq!(queue_ms, 300);
        // An old retransmission must not advance the RTP clock by 13 hours.
        monitor.observe(
            stamp(1 << 17),
            1000,
            false,
            start + Duration::from_millis(1310),
        );
        let Some(Message::Uplink { queue_ms, .. }) = monitor.observe(
            stamp(100 * (1 << 18)),
            1000,
            true,
            start + Duration::from_secs(100),
        ) else {
            panic!("missing feedback");
        };
        assert_eq!(
            queue_ms, 0,
            "a paused interval/new encoder clock must not look like congestion"
        );
    }
}
