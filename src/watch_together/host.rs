use super::{
    Emitter, Event, Snapshot,
    media::{Encoder, Tracks},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
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

struct Peer {
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
            let sender = connection
                .add_track(track)
                .await
                .map_err(|_| "Could not attach a media track")?;
            // Drain RTCP so NACK feedback and sender reports continue to run.
            rtcp.push(tokio::spawn(async move {
                let mut buffer = [0; 1500];
                while sender.read(&mut buffer).await.is_ok() {}
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
        connection,
        pending: vec![],
        rtcp,
    })
}

pub(super) async fn run(
    endpoint: String,
    key: String,
    ffmpeg: PathBuf,
    mut snapshots: watch::Receiver<Snapshot>,
    mut stopped: oneshot::Receiver<()>,
    emit: Emitter,
) -> Result<(), String> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let connect = tokio_tungstenite::connect_async_with_config(&endpoint, Some(config), false);
    let (mut socket, _) = tokio::select! {
        _ = &mut stopped => return Ok(()),
        result = tokio::time::timeout(Duration::from_secs(15), connect) => result.map_err(|_| "The watch service did not respond")?.map_err(|_| "Could not connect to the watch service. Check its address and internet connection.")?,
    };
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&Message::Create { key })
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
                "--get-url",
                "--format",
                "bestvideo[height<=720]+bestaudio/best",
                "--",
                &initial.source,
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let output = tokio::select! {
            _ = &mut stopped => return Ok(()),
            output = tokio::time::timeout(Duration::from_secs(35), command.output()) => output.map_err(|_| "Website video resolution timed out")?.map_err(|_| "Could not run yt-dlp")?,
        };
        let text = String::from_utf8(output.stdout)
            .map_err(|_| "Website returned an invalid media URL")?;
        let mut lines = text
            .lines()
            .filter(|s| s.starts_with("https://") || s.starts_with("http://"));
        initial.source = lines
            .next()
            .filter(|_| output.status.success())
            .ok_or("Could not resolve this website video for sharing")?
            .to_owned();
        initial.audio_source = lines.next().map(str::to_owned);
    }
    let resolved_source = initial.source.clone();
    let resolved_audio = initial.audio_source.clone();
    let tracks = Tracks::new();
    let mut peers: HashMap<String, Peer> = HashMap::new();
    let mut ice = vec![];
    let (outbound, mut outgoing) = mpsc::channel::<Message>(128);
    let mut encoder: Option<Encoder> = None;
    let mut preview_at: Option<(f64, usize)> = None;
    let mut snapshot = initial;
    snapshots.borrow_and_update();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut last_state = Instant::now() - Duration::from_secs(1);
    let mut created = false;
    let mut handshake_deadline = Box::pin(tokio::time::sleep(Duration::from_secs(15)));
    let result = async {
        loop {
            tokio::select! {
                _ = &mut stopped => { break; }
                _ = &mut handshake_deadline, if !created => return Err("The watch service did not create a session".into()),
                changed = snapshots.changed() => {
                    if changed.is_err() { break; }
                    snapshot = snapshots.borrow_and_update().clone();
                    snapshot.source = resolved_source.clone(); snapshot.audio_source = resolved_audio.clone();
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
                    let playing = !snapshot.state.paused && !snapshot.state.buffering && !peers.is_empty();
                    if let Some(e) = &mut encoder && e.completed()? { encoder = None; }
                    let drifted = encoder.as_ref().is_some_and(|e| !e.preview && ((e.position + e.started.elapsed().as_secs_f64() * e.speed - snapshot.state.position).abs() > 1.25 || (e.speed - snapshot.state.speed).abs() > 0.001));
                    if drifted || encoder.as_ref().is_some_and(|e| e.preview == playing) { encoder = None; }
                    if playing {
                        preview_at = None;
                        if encoder.is_none() { encoder = Some(Encoder::start(&ffmpeg, &snapshot, &tracks).await?); }
                    } else if snapshot.state.paused && !snapshot.state.buffering && snapshot.video && ready > 0
                        && preview_at.is_none_or(|(position, count)| (position - snapshot.state.position).abs() > 0.1 || count != ready) {
                        encoder = None;
                        encoder = Some(Encoder::start(&ffmpeg, &snapshot, &tracks).await?);
                        preview_at = Some((snapshot.state.position, ready));
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
                                Message::Created { url, ice_servers } if !created => { ice = ice_servers; created = true; emit.send(Event::Created(url)); }
                                Message::PeerJoined { peer: id } if peers.len() < MAX_VIEWERS && !peers.contains_key(&id) => {
                                    let viewer = peer(&id, &ice, &tracks, &snapshot, outbound.clone()).await?;
                                    peers.insert(id, viewer); emit.send(Event::Viewers(peers.len()));
                                }
                                Message::PeerLeft { peer } => { if let Some(p) = peers.remove(&peer) { p.close().await; } emit.send(Event::Viewers(peers.len())); }
                                Message::Answer { peer, sdp } => if let Some(viewer) = peers.get_mut(&peer) {
                                    let accepted = match RTCSessionDescription::answer(sdp) {
                                        Ok(answer) => viewer.connection.set_remote_description(answer).await.is_ok(),
                                        Err(_) => false,
                                    };
                                    if accepted {
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
