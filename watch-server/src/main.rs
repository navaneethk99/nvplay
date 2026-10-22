mod relay;
use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message as WsMessage, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use rand::RngCore;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, Semaphore, mpsc};
use watch_protocol::{Access, IceServer, MAX_MESSAGE, MAX_VIEWERS, Message, Playback};

type Tx = mpsc::Sender<Message>;
struct Viewer {
    tx: Tx,
    media: Option<relay::Connection>,
}
struct Room {
    token: String,
    relay: Option<relay::Relay>,
    uplink_feedback: bool,
    host: Tx,
    viewers: HashMap<String, Viewer>,
    state: Option<Playback>,
    access: Access,
}
struct Service {
    connections: Arc<Semaphore>,
    rooms: Mutex<HashMap<String, Room>>,
    public_url: String,
    key: String,
    turn_urls: Vec<String>,
    turn_secret: String,
    max_rooms: usize,
    idle_since: std::sync::Mutex<Instant>,
    sleep_claim: std::sync::Mutex<Option<Instant>>,
    idle_seconds: u64,
}
fn secret() -> String {
    let mut bytes = [0; 24];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
fn equal_secret(a: &str, b: &str) -> bool {
    // Verify a MAC rather than comparing user-controlled bearer secrets directly.
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(b"nvplay-watch-auth").unwrap();
    mac.update(a.as_bytes());
    let tag = mac.finalize().into_bytes();
    let mut expected = Hmac::<sha1::Sha1>::new_from_slice(b"nvplay-watch-auth").unwrap();
    expected.update(b.as_bytes());
    expected.verify_slice(&tag).is_ok()
}
impl Service {
    fn sleeping(&self) -> bool {
        self.sleep_claim
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < Duration::from_secs(90))
    }
    fn ice_servers(&self) -> Vec<IceServer> {
        if self.turn_urls.is_empty() {
            return vec![];
        }
        // 24 hours accommodates a movie marathon. The server secret never leaves here.
        let expiry = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 86400;
        let username = format!("{expiry}:nvplay-{}", secret());
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(self.turn_secret.as_bytes()).unwrap();
        mac.update(username.as_bytes());
        vec![IceServer {
            urls: self.turn_urls.clone(),
            username,
            credential: STANDARD.encode(mac.finalize().into_bytes()),
        }]
    }
}
async fn socket(State(service): State<Arc<Service>>, ws: WebSocketUpgrade) -> impl IntoResponse {
    let Ok(permit) = service.connections.clone().try_acquire_owned() else {
        return axum::http::StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |ws| async move {
            let _permit = permit;
            session(service, ws).await;
        })
        .into_response()
}
async fn next_message(socket: &mut WebSocket) -> Option<Message> {
    loop {
        match socket.recv().await? {
            Ok(WsMessage::Text(text)) => return serde_json::from_str(&text).ok(),
            Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) => continue,
            _ => return None,
        }
    }
}
async fn reject(socket: &mut WebSocket, message: &str) {
    let msg = Message::Error {
        message: message.into(),
    };
    let _ = socket
        .send(WsMessage::Text(serde_json::to_string(&msg).unwrap().into()))
        .await;
    let _ = socket.close().await;
}
async fn session(service: Arc<Service>, mut socket: WebSocket) {
    let Ok(Some(first)) =
        tokio::time::timeout(Duration::from_secs(10), next_message(&mut socket)).await
    else {
        return;
    };
    let (tx, mut rx) = mpsc::channel::<Message>(64);
    let (room_id, peer_id) = match first {
        Message::Create { key, access } => {
            if !equal_secret(&key, &service.key) {
                reject(&mut socket, "Host authentication failed").await;
                return;
            }
            let mut rooms = service.rooms.lock().await;
            if service.sleeping() {
                drop(rooms);
                reject(
                    &mut socket,
                    "The watch service is going to sleep. Try creating a session again shortly.",
                )
                .await;
                return;
            }
            if rooms.len() >= service.max_rooms {
                drop(rooms);
                reject(&mut socket, "The watch service is full").await;
                return;
            }
            let room_id = secret();
            let token = secret();
            let url = format!("{}/watch/{room_id}#{token}", service.public_url);
            rooms.insert(
                room_id.clone(),
                Room {
                    token,
                    relay: None,
                    uplink_feedback: false,
                    host: tx.clone(),
                    viewers: HashMap::new(),
                    state: None,
                    access,
                },
            );
            let _ = tx.try_send(Message::Created {
                url,
                ice_servers: service.ice_servers(),
            });
            (room_id, None)
        }
        Message::Join {
            room,
            token,
            client_offers,
        } => {
            let mut rooms = service.rooms.lock().await;
            let Some(session) = rooms
                .get_mut(&room)
                .filter(|r| equal_secret(&r.token, &token))
            else {
                drop(rooms);
                reject(&mut socket, "This viewing link has expired or is invalid").await;
                return;
            };
            if !session.access.allow_join {
                drop(rooms);
                reject(
                    &mut socket,
                    "The host has locked this watch party to new viewers",
                )
                .await;
                return;
            }
            if session.viewers.len() >= MAX_VIEWERS {
                drop(rooms);
                reject(&mut socket, "This watch party is full (six viewers)").await;
                return;
            }
            let peer = secret();
            if session
                .host
                .try_send(Message::PeerJoined { peer: peer.clone() })
                .is_err()
            {
                drop(rooms);
                reject(&mut socket, "The host is unavailable").await;
                return;
            }
            let client_offers = client_offers && session.relay.is_some();
            let _ = tx.try_send(Message::Joined {
                peer: peer.clone(),
                client_offers,
                ice_servers: service.ice_servers(),
                access: session.access,
            });
            if let Some(state) = &session.state {
                let _ = tx.try_send(Message::State {
                    state: state.clone(),
                });
            }
            let media = if let Some(relay) = &session.relay {
                match relay
                    .subscribe(&peer, tx.clone(), session.host.clone(), client_offers)
                    .await
                {
                    Ok(connection) => Some(connection),
                    Err(_) => {
                        drop(rooms);
                        reject(
                            &mut socket,
                            "Could not start the video connection. Try again.",
                        )
                        .await;
                        return;
                    }
                }
            } else {
                None
            };
            session.viewers.insert(
                peer.clone(),
                Viewer {
                    tx: tx.clone(),
                    media,
                },
            );
            (room, Some(peer))
        }
        _ => {
            reject(&mut socket, "Create or join a session first").await;
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut last_seen = Instant::now();
    let started = Instant::now();
    let mut budget = (Instant::now(), 0_u32);
    loop {
        tokio::select! {
            message = rx.recv() => {
                let Some(message) = message else { break; };
                let ended = matches!(message, Message::Ended | Message::Error { .. });
                if sink.send(WsMessage::Text(serde_json::to_string(&message).unwrap().into())).await.is_err() || ended { break; }
            }
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > Duration::from_secs(45) || started.elapsed() > Duration::from_secs(23 * 3600) { break; }
                if sink.send(WsMessage::Ping(vec![].into())).await.is_err() { break; }
            }
            incoming = stream.next() => {
                let Some(Ok(incoming)) = incoming else { break; };
                last_seen = Instant::now();
                let WsMessage::Text(text) = incoming else {
                    if matches!(incoming, WsMessage::Close(_)) { break; }
                    continue;
                };
                if budget.0.elapsed() >= Duration::from_secs(1) { budget = (Instant::now(), 0); }
                budget.1 += 1;
                if budget.1 > 40 { break; }
                let Ok(message) = serde_json::from_str::<Message>(&text) else { break; };
                if matches!(message, Message::Ping) { let _ = tx.try_send(Message::Pong); continue; }
                let mut rooms = service.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else { break; };
                match (&peer_id, message) {
                    (None, Message::State { state }) if state.valid() => {
                        room.state = Some(state.clone());
                        for viewer in room.viewers.values() { let _ = viewer.tx.try_send(Message::State { state: state.clone() }); }
                    }
                    (None, Message::Uplink { bitrate: 0, queue_ms: 0 }) if room.relay.is_none() => { room.uplink_feedback = true; }
                    (None, Message::Offer { peer, sdp }) if peer == "__publisher" && room.relay.is_none() => {
                        match relay::Relay::publish(sdp, room.host.clone(), room.uplink_feedback).await {
                            Ok(relay) => {
                                for (id, viewer) in &mut room.viewers {
                                    viewer.media = relay.subscribe(id, viewer.tx.clone(), room.host.clone(), false).await.ok();
                                }
                                room.relay = Some(relay);
                            }
                            Err(_) => { let _ = tx.try_send(Message::Error { message: "Could not establish the streaming uplink".into() }); }
                        }
                    }
                    (None, Message::Candidate { peer, candidate }) if peer == "__publisher" => {
                        if let Some(relay) = &mut room.relay { relay.publisher.candidate(candidate).await; }
                    }
                    (None, message @ (Message::Offer { .. } | Message::Candidate { .. })) => {
                        let peer = match &message { Message::Offer { peer, .. } | Message::Candidate { peer, .. } => peer, _ => unreachable!() };
                        if let Some(viewer) = room.viewers.get(peer) { let _ = viewer.tx.try_send(message); }
                    }
                    (None, Message::PeerLeft { peer }) => {
                        if let Some(viewer) = room.viewers.remove(&peer) {
                            let _ = viewer.tx.try_send(Message::Error { message: "Your video connection could not be established. Reopen the link to try again.".into() });
                        }
                    }
                    (None, Message::Access { access }) => {
                        room.access = access;
                        for viewer in room.viewers.values() { let _ = viewer.tx.try_send(Message::Access { access }); }
                    }
                    (None, Message::Ended) => break,
                    (Some(id), Message::Offer { sdp, .. }) if room.relay.is_some() => {
                        if let Some(viewer) = room.viewers.get_mut(id) && let Some(media) = &mut viewer.media
                            && media.offer(sdp, id, viewer.tx.clone()).await.is_err() { break; }
                    }
                    (Some(id), Message::Answer { sdp, .. }) if room.relay.is_some() => {
                        if let Some(media) = room.viewers.get_mut(id).and_then(|v| v.media.as_mut())
                            && media.answer(sdp).await.is_err() { break; }
                    }
                    (Some(id), Message::Candidate { candidate, .. }) if room.relay.is_some() => {
                        if let Some(media) = room.viewers.get_mut(id).and_then(|v| v.media.as_mut()) { media.candidate(candidate).await; }
                    }
                    (Some(id), Message::Answer { sdp, .. }) => {
                        if room.host.try_send(Message::Answer { peer: id.clone(), sdp }).is_err() { break; }
                    }
                    (Some(id), Message::Candidate { candidate, .. }) => {
                        if room.host.try_send(Message::Candidate { peer: id.clone(), candidate }).is_err() { break; }
                    }
                    (Some(_), Message::Control { control }) if control.valid() => {
                        if room.access.permits(&control) {
                            if room.host.try_send(Message::Control { control }).is_err() { break; }
                        } else {
                            let _ = tx.try_send(Message::Denied { message: "The host has disabled this playback control.".into() });
                        }
                    }
                    (_, Message::Pong) => {},
                    _ => break,
                }
            }
        }
    }
    let mut rooms = service.rooms.lock().await;
    if let Some(peer) = peer_id {
        if let Some(room) = rooms.get_mut(&room_id) {
            if let Some(viewer) = room.viewers.remove(&peer)
                && let Some(media) = viewer.media
            {
                media.close().await;
            }
            let _ = room.host.try_send(Message::PeerLeft { peer });
        }
    } else if let Some(room) = rooms.remove(&room_id) {
        if rooms.is_empty() {
            *service.idle_since.lock().unwrap() = Instant::now();
        }
        for viewer in room.viewers.into_values() {
            let _ = viewer.tx.try_send(Message::Ended);
            if let Some(media) = viewer.media {
                media.close().await;
            }
        }
        if let Some(relay) = room.relay {
            relay.close().await;
        }
    }
}
// These routes are blocked by Caddy and bound to loopback on the VM. Host
// authentication remains required, even for local maintenance processes.
async fn activity(State(service): State<Arc<Service>>, headers: HeaderMap) -> impl IntoResponse {
    idle_status(service, headers, false).await
}
async fn claim_idle(State(service): State<Arc<Service>>, headers: HeaderMap) -> impl IntoResponse {
    idle_status(service, headers, true).await
}
async fn idle_status(
    service: Arc<Service>,
    headers: HeaderMap,
    claim: bool,
) -> axum::response::Response {
    let authorized = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .is_some_and(|key| !service.key.is_empty() && equal_secret(key, &service.key));
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Sharing creation and the shutdown claim use the same room lock. A new
    // session cannot slip between the idle check and the deallocation request.
    let rooms = service.rooms.lock().await;
    let idle = service.idle_since.lock().unwrap().elapsed().as_secs();
    let shutdown = claim && rooms.is_empty() && idle >= service.idle_seconds && !service.sleeping();
    if shutdown {
        *service.sleep_claim.lock().unwrap() = Some(Instant::now());
    }
    Json(serde_json::json!({"active_sessions": rooms.len(), "idle_seconds": idle, "shutdown": shutdown})).into_response()
}
fn app(service: Arc<Service>) -> Router {
    Router::new()
        .route("/health", get(|State(s): State<Arc<Service>>| async move {
            if s.sleeping() { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK }
        }))
        .route("/admin/activity", get(activity))
        .route("/admin/idle", post(claim_idle))
        .route("/watch/{room}", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/viewer.js", get(|| async { ([("content-type", "text/javascript; charset=utf-8")], include_str!("../web/viewer.js")) }))
        .route("/ws", get(socket))
        .layer(axum::middleware::from_fn(|request: axum::extract::Request, next: axum::middleware::Next| async move {
            let mut response = next.run(request).await;
            for (key, value) in [
                ("referrer-policy", "no-referrer"),
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
                ("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'"),
            ] { response.headers_mut().insert(axum::http::HeaderName::from_static(key), value.parse().unwrap()); }
            response
        }))
        .with_state(service)
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let public_url = std::env::var("NVPLAY_WATCH_PUBLIC_URL")
        .unwrap_or_else(|_| "http://localhost:8787".into())
        .trim_end_matches('/')
        .to_owned();
    let parsed = url::Url::parse(&public_url)?;
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(parsed.scheme() == "https" || (local && parsed.scheme() == "http"))
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("PUBLIC_URL must be an HTTPS origin (HTTP allowed on localhost)".into());
    }
    let key = std::env::var("NVPLAY_WATCH_HOST_KEY").unwrap_or_default();
    let turn_secret = std::env::var("NVPLAY_TURN_SECRET").unwrap_or_default();
    let turn_urls: Vec<String> = std::env::var("NVPLAY_TURN_URLS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if !local && (key.len() < 32 || turn_secret.len() < 32 || turn_urls.is_empty()) {
        return Err("Public hosting requires a host key and TURN secret of at least 32 characters, and NVPLAY_TURN_URLS".into());
    }
    if turn_urls
        .iter()
        .any(|s| !(s.starts_with("turn:") || s.starts_with("turns:")))
        || (!turn_urls.is_empty() && turn_secret.is_empty())
    {
        return Err("Configure valid turn:/turns: URLs and a TURN secret".into());
    }
    let service = Arc::new(Service {
        connections: Arc::new(Semaphore::new(1024)),
        rooms: Mutex::default(),
        public_url,
        key,
        turn_urls,
        turn_secret,
        max_rooms: 100,
        idle_since: std::sync::Mutex::new(Instant::now()),
        sleep_claim: std::sync::Mutex::new(None),
        idle_seconds: std::env::var("NVPLAY_WATCH_IDLE_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(900)
            .max(60),
    });
    let bind = std::env::var("NVPLAY_WATCH_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    eprintln!("nvplay watch service listening on {bind}");
    axum::serve(listener, app(service))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn idle_shutdown_requires_auth_expired_grace_and_no_sessions() {
        let service = Arc::new(Service {
            connections: Arc::new(Semaphore::new(10)),
            rooms: Mutex::default(),
            public_url: String::new(),
            key: "test-host-key".into(),
            turn_urls: vec![],
            turn_secret: String::new(),
            max_rooms: 10,
            idle_since: std::sync::Mutex::new(Instant::now()),
            sleep_claim: std::sync::Mutex::new(None),
            idle_seconds: 900,
        });
        assert_eq!(
            idle_status(service.clone(), HeaderMap::new(), true)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-host-key".parse().unwrap());
        idle_status(service.clone(), headers.clone(), true).await;
        assert!(
            !service.sleeping(),
            "startup grace must protect a freshly started VM"
        );
        *service.idle_since.lock().unwrap() = Instant::now() - Duration::from_secs(901);
        let (tx, _rx) = mpsc::channel(10);
        service.rooms.lock().await.insert(
            "room".into(),
            Room {
                token: secret(),
                relay: None,
                uplink_feedback: false,
                host: tx,
                viewers: HashMap::new(),
                state: None,
                access: Access::default(),
            },
        );
        idle_status(service.clone(), headers.clone(), true).await;
        assert!(
            !service.sleeping(),
            "a host session is active even with no viewers"
        );
        service.rooms.lock().await.clear();
        idle_status(service.clone(), headers.clone(), false).await;
        assert!(
            !service.sleeping(),
            "read-only status must not claim shutdown"
        );
        idle_status(service.clone(), headers.clone(), true).await;
        assert!(service.sleeping());
        let response = idle_status(service.clone(), headers, true).await;
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["shutdown"], false,
            "only one deallocation lease can be issued"
        );
        *service.sleep_claim.lock().unwrap() = Some(Instant::now() - Duration::from_secs(91));
        assert!(
            !service.sleeping(),
            "failed cloud requests must not block new sessions forever"
        );
    }

    #[test]
    fn invite_secrets_are_unpredictable_and_compared_exactly() {
        let a = secret();
        let b = secret();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(equal_secret(&a, &a));
        assert!(!equal_secret(&a, &b));
        assert!(!equal_secret(&a, ""));
    }
    #[test]
    fn temporary_turn_credentials_match_coturn_rest_auth() {
        let service = Service {
            connections: Arc::new(Semaphore::new(1024)),
            rooms: Mutex::default(),
            public_url: String::new(),
            key: String::new(),
            turn_urls: vec!["turn:localhost:3478".into()],
            turn_secret: "test-secret".into(),
            max_rooms: 1,
            idle_since: std::sync::Mutex::new(Instant::now()),
            sleep_claim: std::sync::Mutex::new(None),
            idle_seconds: 900,
        };
        let ice = service.ice_servers().remove(0);
        let expiry: u64 = ice.username.split(':').next().unwrap().parse().unwrap();
        assert!(
            expiry
                > SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
        );
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(b"test-secret").unwrap();
        mac.update(ice.username.as_bytes());
        mac.verify_slice(&STANDARD.decode(ice.credential).unwrap())
            .unwrap();
    }
    #[tokio::test]
    async fn sessions_authenticate_isolate_route_controls_and_expire() {
        use tokio_tungstenite::{connect_async, tungstenite::Message as Wire};
        type Client = tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >;
        async fn send(client: &mut Client, msg: Message) {
            client
                .send(Wire::Text(serde_json::to_string(&msg).unwrap().into()))
                .await
                .unwrap();
        }
        async fn receive(client: &mut Client) -> Message {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    match client.next().await.unwrap().unwrap() {
                        Wire::Text(text) => return serde_json::from_str(&text).unwrap(),
                        Wire::Ping(data) => client.send(Wire::Pong(data)).await.unwrap(),
                        other => panic!("unexpected message {other:?}"),
                    }
                }
            })
            .await
            .unwrap()
        }
        let service = Arc::new(Service {
            connections: Arc::new(Semaphore::new(1024)),
            rooms: Mutex::default(),
            public_url: "https://watch.example.test".into(),
            key: "host-secret".into(),
            turn_urls: vec![],
            turn_secret: String::new(),
            max_rooms: 10,
            idle_since: std::sync::Mutex::new(Instant::now()),
            sleep_claim: std::sync::Mutex::new(None),
            idle_seconds: 900,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app(service)).await.unwrap();
        });
        let (mut wrong, _) = connect_async(&endpoint).await.unwrap();
        send(
            &mut wrong,
            Message::Create {
                key: "wrong".into(),
                access: Access::default(),
            },
        )
        .await;
        assert!(matches!(receive(&mut wrong).await, Message::Error { .. }));
        let (mut host, _) = connect_async(&endpoint).await.unwrap();
        send(
            &mut host,
            Message::Create {
                key: "host-secret".into(),
                access: Access::default(),
            },
        )
        .await;
        let Message::Created { url, .. } = receive(&mut host).await else {
            panic!()
        };
        let invite = url::Url::parse(&url).unwrap();
        let room = invite
            .path_segments()
            .unwrap()
            .next_back()
            .unwrap()
            .to_owned();
        let token = invite.fragment().unwrap().to_owned();
        let (mut bad, _) = connect_async(&endpoint).await.unwrap();
        send(
            &mut bad,
            Message::Join {
                client_offers: false,
                room: room.clone(),
                token: "wrong".into(),
            },
        )
        .await;
        assert!(matches!(receive(&mut bad).await, Message::Error { .. }));
        let mut viewers = vec![];
        let mut ids = vec![];
        for _ in 0..2 {
            let (mut viewer, _) = connect_async(&endpoint).await.unwrap();
            send(
                &mut viewer,
                Message::Join {
                    client_offers: false,
                    room: room.clone(),
                    token: token.clone(),
                },
            )
            .await;
            let Message::Joined { peer, .. } = receive(&mut viewer).await else {
                panic!()
            };
            assert!(matches!(
                receive(&mut host).await,
                Message::PeerJoined { .. }
            ));
            ids.push(peer);
            viewers.push(viewer);
        }
        // A viewer cannot impersonate another participant in signaling messages.
        send(
            &mut viewers[0],
            Message::Answer {
                peer: ids[1].clone(),
                sdp: "test-answer".into(),
            },
        )
        .await;
        let Message::Answer { peer, .. } = receive(&mut host).await else {
            panic!()
        };
        assert_eq!(peer, ids[0]);
        send(
            &mut viewers[1],
            Message::Control {
                control: watch_protocol::Control::Seek { position: 30.0 },
            },
        )
        .await;
        assert!(matches!(
            receive(&mut host).await,
            Message::Control {
                control: watch_protocol::Control::Seek { position: 30.0 }
            }
        ));
        // Permissions change live, are enforced even for forged browser commands,
        // and locking a link does not evict existing viewers.
        let restricted = Access {
            play_pause: false,
            seek: false,
            allow_join: false,
        };
        send(&mut host, Message::Access { access: restricted }).await;
        for viewer in &mut viewers {
            assert!(
                matches!(receive(viewer).await, Message::Access { access } if access == restricted)
            );
        }
        for control in [
            watch_protocol::Control::Play {},
            watch_protocol::Control::Pause {},
            watch_protocol::Control::Seek { position: 40.0 },
        ] {
            send(&mut viewers[0], Message::Control { control }).await;
            assert!(matches!(
                receive(&mut viewers[0]).await,
                Message::Denied { .. }
            ));
        }
        send(&mut host, Message::Ping).await;
        assert!(matches!(receive(&mut host).await, Message::Pong));
        let (mut locked, _) = connect_async(&endpoint).await.unwrap();
        send(
            &mut locked,
            Message::Join {
                client_offers: false,
                room: room.clone(),
                token: token.clone(),
            },
        )
        .await;
        assert!(
            matches!(receive(&mut locked).await, Message::Error { message } if message.contains("locked"))
        );
        let play_only = Access {
            play_pause: true,
            seek: false,
            allow_join: true,
        };
        send(&mut host, Message::Access { access: play_only }).await;
        for viewer in &mut viewers {
            assert!(matches!(receive(viewer).await, Message::Access { .. }));
        }
        send(
            &mut viewers[0],
            Message::Control {
                control: watch_protocol::Control::Pause {},
            },
        )
        .await;
        assert!(matches!(
            receive(&mut host).await,
            Message::Control {
                control: watch_protocol::Control::Pause {}
            }
        ));
        send(
            &mut viewers[0],
            Message::Control {
                control: watch_protocol::Control::Seek { position: 40.0 },
            },
        )
        .await;
        assert!(matches!(
            receive(&mut viewers[0]).await,
            Message::Denied { .. }
        ));
        send(
            &mut host,
            Message::Access {
                access: Access::default(),
            },
        )
        .await;
        for viewer in &mut viewers {
            assert!(matches!(receive(viewer).await, Message::Access { .. }));
        }
        let state = Playback {
            position: 30.0,
            duration: 60.0,
            paused: true,
            buffering: false,
            speed: 1.0,
            title: "Movie".into(),
        };
        send(
            &mut host,
            Message::State {
                state: state.clone(),
            },
        )
        .await;
        for viewer in &mut viewers {
            let Message::State { state: actual } = receive(viewer).await else {
                panic!()
            };
            assert_eq!(actual, state);
        }
        send(
            &mut host,
            Message::Offer {
                peer: ids[1].clone(),
                sdp: "offer-for-second-only".into(),
            },
        )
        .await;
        assert!(matches!(
            receive(&mut viewers[1]).await,
            Message::Offer { .. }
        ));
        send(&mut host, Message::Ended).await;
        for viewer in &mut viewers {
            assert!(matches!(receive(viewer).await, Message::Ended));
        }
        let (mut expired, _) = connect_async(&endpoint).await.unwrap();
        send(
            &mut expired,
            Message::Join {
                room,
                token,
                client_offers: false,
            },
        )
        .await;
        assert!(matches!(receive(&mut expired).await, Message::Error { .. }));
        server.abort();
    }
}
