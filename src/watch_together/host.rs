use super::{
    Emitter, Event, Snapshot,
    media::{AdaptiveRate, Encoder, HlsOptions, Tracks, video_bitrate},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::{Message as WsMessage, protocol::WebSocketConfig};
use watch_protocol::{IceServer, MAX_MESSAGE, MAX_VIEWERS, Message};
use webrtc::{
    api::{
        APIBuilder, interceptor_registry::register_default_interceptors, media_engine::MediaEngine,
    },
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{
        RTCPeerConnection, configuration::RTCConfiguration,
        sdp::session_description::RTCSessionDescription,
    },
    track::track_local::TrackLocal,
};

fn media_url(metadata: &serde_json::Value) -> Option<String> {
    let value = metadata["url"].as_str()?;
    url::Url::parse(value)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())?;
    Some(value.to_owned())
}
fn media_headers(format: &serde_json::Value, metadata: &serde_json::Value) -> String {
    let mut headers = metadata["http_headers"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(overrides) = format["http_headers"].as_object() {
        headers.extend(overrides.clone());
    }
    let mut result = String::new();
    {
        for (name, value) in &headers {
            let Some(value) = value.as_str() else {
                continue;
            };
            if name.is_empty()
                || name.len() > 128
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                || value.contains(['\r', '\n', '\0'])
                || value.len() > 8192
                || result.len() + name.len() + value.len() > 32768
            {
                continue;
            }
            result.push_str(name);
            result.push_str(": ");
            result.push_str(value);
            result.push_str("\r\n");
        }
    }
    result
}

struct Peer {
    loss: Arc<AtomicU8>,
    estimate: Arc<AtomicU64>,
    connection: Arc<RTCPeerConnection>,
    pending: Vec<webrtc::ice_transport::ice_candidate::RTCIceCandidateInit>,
    rtcp: Vec<tokio::task::JoinHandle<()>>,
}
impl Peer {
    async fn close(self) {
        let _ = self.connection.close().await;
        for task in self.rtcp {
            task.abort();
        }
    }
}
async fn peer(
    id: &str,
    ice: &[IceServer],
    tracks: &Tracks,
    snapshot: &Snapshot,
    outbound: mpsc::Sender<Message>,
) -> Result<Peer, String> {
    let mut engine = MediaEngine::default();
    engine
        .register_default_codecs()
        .map_err(|_| "Could not initialize WebRTC codecs")?;
    let registry = register_default_interceptors(Registry::new(), &mut engine)
        .map_err(|_| "Could not initialize WebRTC transport")?;
    engine
        .register_header_extension(
            webrtc::rtp_transceiver::rtp_codec::RTCRtpHeaderExtensionCapability {
                uri: webrtc::sdp::extmap::ABS_SEND_TIME_URI.into(),
            },
            webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video,
            None,
        )
        .map_err(|_| "Could not configure network timing")?;
    let api = APIBuilder::new()
        .with_media_engine(engine)
        .with_interceptor_registry(registry)
        .build();
    let connection = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers: ice
                .iter()
                .map(|i| RTCIceServer {
                    urls: i.urls.clone(),
                    username: i.username.clone(),
                    credential: i.credential.clone(),
                })
                .collect(),
            ..Default::default()
        })
        .await
        .map_err(|_| "Could not create a viewer connection")?,
    );
    let candidate_tx = outbound.clone();
    let peer_id = id.to_owned();
    connection.on_ice_candidate(Box::new(move |candidate| {
        let tx = candidate_tx.clone();
        let peer = peer_id.clone();
        Box::pin(async move {
            if let Some(candidate) = candidate
                .and_then(|c| c.to_json().ok())
                .and_then(|c| serde_json::to_value(c).ok())
            {
                let _ = tx.send(Message::Candidate { peer, candidate }).await;
            }
        })
    }));
    let loss = Arc::new(AtomicU8::new(0));
    let estimate = Arc::new(AtomicU64::new(0));
    let mut rtcp = vec![];
    let result = async {
        let mut locals: Vec<Arc<dyn TrackLocal + Send + Sync>> = vec![];
        if snapshot.video {
            locals.push(tracks.video.clone());
        }
        if snapshot.audio {
            locals.push(tracks.audio.clone());
        }
        for track in locals {
            let video = track.kind() == webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video;
            let sender = connection
                .add_track(track)
                .await
                .map_err(|_| "Could not attach a media track")?;
            let loss = loss.clone();
            let estimate = estimate.clone();
            rtcp.push(tokio::spawn(async move {
                while let Ok((packets, _)) = sender.read_rtcp().await {
                    if !video { continue; }
                    for packet in packets {
                        if let Some(report) = packet.as_any().downcast_ref::<webrtc::rtcp::receiver_report::ReceiverReport>() {
                            for reception in &report.reports { loss.fetch_max(reception.fraction_lost, Ordering::Relaxed); }
                        }
                        if let Some(remb) = packet.as_any().downcast_ref::<webrtc::rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate>()
                            && remb.bitrate.is_finite() && remb.bitrate > 0.0 {
                            estimate.store(remb.bitrate as u64, Ordering::Relaxed);
                        }
                    }
                }
            }));
        }
        let offer = connection
            .create_offer(None)
            .await
            .map_err(|_| "Could not create a WebRTC offer")?;
        let sdp = offer.sdp.clone();
        connection
            .set_local_description(offer)
            .await
            .map_err(|_| "Could not start ICE negotiation")?;
        outbound
            .send(Message::Offer {
                peer: id.to_owned(),
                sdp,
            })
            .await
            .map_err(|_| "The watch service disconnected")?;
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = result {
        let _ = connection.close().await;
        for task in rtcp {
            task.abort();
        }
        return Err(error);
    }
    Ok(Peer {
        loss,
        estimate,
        connection,
        pending: vec![],
        rtcp,
    })
}

pub(super) async fn run(
    endpoint: String,
    key: String,
    ffmpeg: Option<PathBuf>,
    media: Option<watch_protocol::Media>,
    mut snapshots: watch::Receiver<Snapshot>,
    mut stopped: oneshot::Receiver<()>,
    emit: Emitter,
) -> Result<(), String> {
    tokio::select! {
        _ = &mut stopped => return Ok(()),
        result = super::azure::ensure_running(&endpoint, &emit) => result?,
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let connect = tokio_tungstenite::connect_async_with_config(&endpoint, Some(config), false);
    let (mut socket, _) = tokio::select! {
        _ = &mut stopped => return Ok(()),
        result = tokio::time::timeout(Duration::from_secs(15), connect) => result.map_err(|_| "The watch service did not respond")?.map_err(|_| "Could not connect to the watch service. Check its address and internet connection.")?,
    };
    if let Some(media) = media {
        return super::direct::run(socket, key, media, snapshots, stopped, emit).await;
    }
    let ffmpeg = ffmpeg.ok_or("Install FFmpeg to broadcast local files")?;
    let access = snapshots.borrow().access;
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&Message::Create {
                media: None,
                key,
                access,
            })
            .unwrap()
            .into(),
        ))
        .await
        .map_err(|_| "The watch service disconnected")?;
    // Resolve website pages on the background worker. Local files and the active
    // Cinejoy HLS playlist can be opened directly by FFmpeg.
    let mut initial = snapshots.borrow().clone();
    if crate::streaming::is_webpage(&initial.source) {
        let ytdlp = crate::streaming::find_tool("NVPLAY_YTDLP", "yt-dlp")
            .ok_or("Install yt-dlp to share website videos")?;
        let mut command = tokio::process::Command::from(crate::streaming::tool_command(&ytdlp));
        command
            .kill_on_drop(true)
            .args([
                "--no-playlist",
                "--no-warnings",
                "--dump-single-json",
                "--skip-download",
                "--check-formats",
                "--socket-timeout", "8", "--retries", "1", "--extractor-retries", "1",
                "--format",
                &if initial.height > 0 {
                    format!("bestvideo[height={0}][vcodec^=avc1]+bestaudio/best[height={0}][vcodec^=avc1]/bestvideo[height={0}]+bestaudio/best[height={0}]/bestvideo[height>{0}]+bestaudio/best[height>{0}]", initial.height)
                } else {
                    "bestvideo+bestaudio/best".into()
                },
                "--",
                &initial.source,
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let output = tokio::select! {
            _ = &mut stopped => return Ok(()),
            output = tokio::time::timeout(Duration::from_secs(35), command.output()) => output.map_err(|_| "Website video resolution timed out")?.map_err(|_| "Could not run yt-dlp")?,
        };
        if output.stdout.len() > 4 * 1024 * 1024 {
            return Err("Website media metadata is too large.".into());
        }
        if !output.status.success() {
            return Err(
                "Could not resolve a playable website video at the current resolution.".into(),
            );
        }
        let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| "The website returned invalid media metadata")?;
        let formats = metadata["requested_formats"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![metadata.clone()]);
        let video = formats
            .iter()
            .find(|f| f["vcodec"].as_str().is_some_and(|c| c != "none"))
            .unwrap_or(&metadata);
        initial.source =
            media_url(video).ok_or("The website did not return a playable video address")?;
        initial.video_headers = media_headers(video, &metadata);
        if let Some(audio) = formats.iter().find(|f| {
            f["acodec"].as_str().is_some_and(|c| c != "none")
                && media_url(f).as_deref() != Some(initial.source.as_str())
        }) {
            initial.audio_source = media_url(audio);
            initial.audio_headers = media_headers(audio, &metadata);
        }
    }
    let resolved_source = initial.source.clone();
    let resolved_audio = initial.audio_source.clone();
    let resolved_video_headers = initial.video_headers.clone();
    let resolved_audio_headers = initial.audio_headers.clone();
    let mut hls = HlsOptions::discover(&ffmpeg).await;
    let tracks = Tracks::new();
    let mut peers: HashMap<String, Peer> = HashMap::new();
    let mut viewers = HashSet::new();
    let (outbound, mut outgoing) = mpsc::channel::<Message>(128);
    let mut encoder: Option<Encoder> = None;
    let mut preview_at: Option<(f64, usize)> = None;
    let mut snapshot = initial;
    let mut rate = AdaptiveRate::new(
        video_bitrate(snapshot.width, snapshot.height),
        Instant::now(),
    );
    snapshot.bitrate = rate.rate;
    snapshots.borrow_and_update();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut last_viewer_at = Instant::now();
    let mut last_state = Instant::now() - Duration::from_secs(1);
    let mut created = false;
    let mut staged_url = None;
    let mut handshake_deadline = Box::pin(tokio::time::sleep(Duration::from_secs(15)));
    let result = async {
        loop {
            tokio::select! {
                _ = &mut stopped => { break; }
                _ = &mut handshake_deadline, if !created || staged_url.is_some() => return Err("The watch service did not establish the streaming uplink".into()),
                changed = snapshots.changed() => {
                    if changed.is_err() { break; }
                    let next = snapshots.borrow_and_update().clone();
                    if next.access != snapshot.access {
                        socket.send(WsMessage::Text(serde_json::to_string(&Message::Access { access: next.access }).unwrap().into())).await.map_err(|_| "The watch service disconnected")?;
                    }
                    snapshot = next;
                    snapshot.bitrate = rate.rate;
                    snapshot.source = resolved_source.clone(); snapshot.audio_source = resolved_audio.clone();
                    snapshot.video_headers = resolved_video_headers.clone(); snapshot.audio_headers = resolved_audio_headers.clone();
                }
                Some(message) = outgoing.recv() => {
                    socket.send(WsMessage::Text(serde_json::to_string(&message).unwrap().into())).await.map_err(|_| "The watch service disconnected")?;
                }
                _ = tick.tick() => {
                    if !created { continue; }
                    if last_state.elapsed() >= Duration::from_millis(250) {
                        socket.send(WsMessage::Text(serde_json::to_string(&Message::State { state: snapshot.state.clone() }).unwrap().into())).await.map_err(|_| "The watch service disconnected")?;
                        last_state = Instant::now();
                    }
                    let ready = peers.values().filter(|p| p.connection.connection_state() == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected).count();
                    let loss = peers.values().map(|p| p.loss.swap(0, Ordering::Relaxed)).max().unwrap_or(0);
                    let estimate = peers.values().map(|p| p.estimate.load(Ordering::Relaxed)).filter(|b| *b > 0).min().unwrap_or(0);
                    if ready > 0 && rate.feedback(loss, estimate, video_bitrate(snapshot.width, snapshot.height), Instant::now()) {
                        snapshot.bitrate = rate.rate;
                        emit.send(Event::Status(format!("Streaming at original resolution · {:.1} Mbps upload", rate.rate as f64 / 1_000_000.0)));
                    }
                    if !viewers.is_empty() { last_viewer_at = Instant::now(); }
                    if let Some(e) = &mut encoder && let Some(position) = e.align_playback()
                        && !snapshot.state.paused && (position - snapshot.state.position).abs() > 0.25 {
                        // A remote source can take seconds to open. Align the host
                        // once to the first transmitted frame instead of keeping a
                        // permanent startup offset between the two movie clocks.
                        e.synchronizing_until = Some(Instant::now() + Duration::from_secs(2));
                        emit.send(Event::Control(watch_protocol::Control::Seek { position }));
                    }
                    let synchronizing = encoder.as_ref().is_some_and(|e| e.synchronizing_until.is_some_and(|until| until > Instant::now()));
                    let audience = !viewers.is_empty() || (encoder.is_some() && last_viewer_at.elapsed() < Duration::from_secs(30));
                    let playing = !snapshot.state.paused && (!snapshot.state.buffering || synchronizing) && audience && ready > 0;
                    if let Some(e) = &mut encoder {
                        match e.completed().await {
                            Ok(true) => encoder = None,
                            Ok(false) => {},
                            Err(_) if hls.fallback_to_software() => {
                                encoder = None; preview_at = None;
                                emit.send(Event::Status("Hardware encoding unavailable; using software encoding".into()));
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    let drifted = encoder.as_ref().is_some_and(|e| !e.preview && !synchronizing && ((e.position + e.started.elapsed().as_secs_f64() * e.speed - snapshot.state.position).abs() > 1.25 || (e.speed - snapshot.state.speed).abs() > 0.001));
                    if let Some(e) = &mut encoder && e.bitrate != snapshot.bitrate { e.set_bitrate(snapshot.bitrate); }
                    if drifted || encoder.as_ref().is_some_and(|e| e.width != snapshot.width || e.height != snapshot.height || e.preview == playing) { encoder = None; }
                    if playing {
                        preview_at = None;
                        if encoder.is_none() { encoder = Some(Encoder::start(&ffmpeg, &snapshot, &tracks, hls).await?); }
                    } else if snapshot.state.paused && !snapshot.state.buffering && snapshot.video && ready > 0 && !viewers.is_empty()
                        && preview_at.is_none_or(|(position, count)| (position - snapshot.state.position).abs() > 0.1 || count != viewers.len()) {
                        encoder = None;
                        encoder = Some(Encoder::start(&ffmpeg, &snapshot, &tracks, hls).await?);
                        preview_at = Some((snapshot.state.position, viewers.len()));
                    }

                }
                incoming = socket.next() => {
                    let Some(Ok(incoming)) = incoming else { return Err("The watch service disconnected. Start a new watch party to reconnect.".into()); };
                    match incoming {
                        WsMessage::Ping(bytes) => { socket.send(WsMessage::Pong(bytes)).await.map_err(|_| "The watch service disconnected")?; continue; }
                        WsMessage::Close(_) => return Err("The watch service closed this session".into()),
                        WsMessage::Text(text) => {
                            let message: Message = serde_json::from_str(&text).map_err(|_| "The watch service sent an invalid message")?;
                            match message {
                                Message::Created { url, ice_servers } if !created => {
                                    outbound.send(Message::Uplink { bitrate: 0, queue_ms: 0 }).await.map_err(|_| "The watch service disconnected")?;
                                    let publisher = peer("__publisher", &ice_servers, &tracks, &snapshot, outbound.clone()).await?;
                                    peers.insert("__publisher".into(), publisher);
                                    created = true; staged_url = Some(url);
                                }
                                Message::PeerJoined { peer: id } if viewers.contains(&id) || viewers.len() < MAX_VIEWERS => {
                                    preview_at = None;
                                    viewers.insert(id); emit.send(Event::Viewers(viewers.len()));
                                }
                                Message::PeerLeft { peer } => { preview_at = None; viewers.remove(&peer); emit.send(Event::Viewers(viewers.len())); }
                                Message::Answer { peer, sdp } => if let Some(viewer) = peers.get_mut(&peer) {
                                    let accepted = match RTCSessionDescription::answer(sdp) {
                                        Ok(answer) => viewer.connection.set_remote_description(answer).await.is_ok(),
                                        Err(_) => false,
                                    };
                                    if accepted {
                                        for sender in viewer.connection.get_senders().await {
                                            let parameters = sender.get_parameters().await;
                                            if let Some(extension) = parameters.rtp_parameters.header_extensions.iter().find(|e| e.uri == webrtc::sdp::extmap::ABS_SEND_TIME_URI) {
                                                tracks.send_time_extension.store(extension.id as u8, Ordering::Relaxed);
                                            }
                                        }
                                        if peer == "__publisher" && let Some(url) = staged_url.take() { emit.send(Event::Created(url)); }
                                        for candidate in viewer.pending.drain(..) { let _ = viewer.connection.add_ice_candidate(candidate).await; }
                                    } else {
                                        // One failed/malformed viewer must not end everyone else's party.
                                        if let Some(viewer) = peers.remove(&peer) { viewer.close().await; }
                                        let _ = outbound.send(Message::PeerLeft { peer }).await;
                                        emit.send(Event::Viewers(peers.len()));
                                    }
                                },
                                Message::Candidate { peer, candidate } => if let Some(viewer) = peers.get_mut(&peer)
                                    && let Ok(candidate) = serde_json::from_value(candidate) {
                                        if viewer.connection.remote_description().await.is_some() { let _ = viewer.connection.add_ice_candidate(candidate).await; }
                                        else if viewer.pending.len() < 64 { viewer.pending.push(candidate); }
                                },
                                Message::Uplink { bitrate, queue_ms } => {
                                    if let Some(publisher) = peers.get("__publisher") {
                                        // Packet loss alone misses a bloated network queue. Only
                                        // constrain bitrate when delivery is falling behind RTP time.
                                        if queue_ms > 120 {
                                            publisher.loss.fetch_max(if queue_ms > 400 { 51 } else { 13 }, Ordering::Relaxed);
                                            publisher.estimate.store(bitrate.saturating_mul(9) / 10, Ordering::Relaxed);
                                        } else { publisher.estimate.store(0, Ordering::Relaxed); }
                                    }
                                }
                                Message::Control { control } if control.valid() => emit.send(Event::Control(control)),
                                Message::Error { message } => return Err(message),
                                Message::Ended => break,
                                _ => {},
                            }
                        }
                        _ => {},
                    }
                }
            }
        }
        Ok(())
    }.await;
    drop(encoder);
    let ended = WsMessage::Text(serde_json::to_string(&Message::Ended).unwrap().into());
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.send(ended)).await;
    for p in peers.into_values() {
        p.close().await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
    result
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[test]
    fn source_headers_are_forwarded_without_header_injection() {
        let metadata = serde_json::json!({"url":"https://media.example/video", "http_headers":{"User-Agent":"video-client", "Referer":"https://example.com/watch", "Bad\r\nHeader":"value", "Injected":"ok\r\nCookie: bad"}});
        let headers = media_headers(&metadata, &metadata);
        assert!(headers.contains("User-Agent: video-client\r\n"));
        assert!(headers.contains("Referer: https://example.com/watch\r\n"));
        assert!(!headers.contains("Injected"));
        assert!(!headers.contains("Bad"));
        assert!(media_url(&metadata).is_some());
        assert!(media_url(&serde_json::json!({"url":"file:///etc/passwd"})).is_none());
    }
}
