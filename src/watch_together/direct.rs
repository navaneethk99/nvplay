//! A control-only session: viewers fetch media from the provider themselves.
use super::{Emitter, Event, Snapshot};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashSet, time::Duration};
use tokio::{
    net::TcpStream,
    sync::{oneshot, watch},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message as WsMessage};
use watch_protocol::{Media, Message};

pub(super) async fn run(
    mut socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    key: String,
    media: Media,
    mut snapshots: watch::Receiver<Snapshot>,
    mut stopped: oneshot::Receiver<()>,
    emit: Emitter,
) -> Result<(), String> {
    let mut snapshot = snapshots.borrow_and_update().clone();
    async fn send(
        socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        message: Message,
    ) -> Result<(), String> {
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&message).unwrap().into(),
            ))
            .await
            .map_err(|_| "The watch service disconnected".into())
    }
    send(
        &mut socket,
        Message::Create {
            key,
            media: Some(media),
            access: snapshot.access,
        },
    )
    .await?;
    let mut viewers = HashSet::new();
    let mut created = false;
    let deadline = tokio::time::sleep(Duration::from_secs(15));
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let result = async {
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                _ = &mut deadline, if !created => return Err("The watch service did not create a session".into()),
                changed = snapshots.changed() => {
                    if changed.is_err() { break; }
                    let next = snapshots.borrow_and_update().clone();
                    if next.access != snapshot.access {
                        send(&mut socket, Message::Access { access: next.access }).await?;
                    }
                    snapshot = next;
                }
                _ = tick.tick(), if created => {
                    send(&mut socket, Message::State { state: snapshot.state.clone() }).await?;
                }
                incoming = socket.next() => {
                    match incoming {
                        Some(Ok(WsMessage::Ping(bytes))) => { socket.send(WsMessage::Pong(bytes)).await.map_err(|_| "The watch service disconnected")?; }
                        Some(Ok(WsMessage::Text(text))) => {
                            let message: Message = serde_json::from_str(&text).map_err(|_| "The watch service sent an invalid message")?;
                            match message {
                                Message::Created { url, .. } if !created => {
                                    created = true;
                                    send(&mut socket, Message::State { state: snapshot.state.clone() }).await?;
                                    emit.send(Event::Created(url));
                                    emit.send(Event::Status("Direct playback · controls synchronized with your device".into()));
                                }
                                Message::PeerJoined { peer } => { viewers.insert(peer); emit.send(Event::Viewers(viewers.len())); }
                                Message::PeerLeft { peer } => { viewers.remove(&peer); emit.send(Event::Viewers(viewers.len())); }
                                Message::Control { control } if control.valid() && snapshot.access.permits(&control) => emit.send(Event::Control(control)),
                                Message::Error { message } => return Err(message),
                                Message::Ended => break,
                                _ => {}
                            }
                        }
                        Some(Ok(WsMessage::Pong(_))) => {},
                        _ => return Err("The watch service disconnected. Start a new watch party to reconnect.".into()),
                    }
                }
            }
        }
        Ok(())
    }.await;
    let _ = tokio::time::timeout(Duration::from_secs(2), send(&mut socket, Message::Ended)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use watch_protocol::{Access, Control, Playback};

    #[tokio::test]
    async fn desktop_direct_session_sends_state_and_controls_without_an_encoder() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
        let media = Media::Youtube {
            id: "dQw4w9WgXcQ".into(),
        };
        let snapshot = Snapshot {
            source: "https://youtu.be/dQw4w9WgXcQ".into(),
            audio_source: None,
            video_headers: String::new(),
            audio_headers: String::new(),
            video: true,
            audio: true,
            width: 1920,
            height: 1080,
            bitrate: 0,
            access: Access::default(),
            state: Playback {
                position: 10.0,
                duration: 60.0,
                paused: true,
                buffering: false,
                speed: 1.0,
                title: "Test".into(),
            },
        };
        let (snapshots, rx) = watch::channel(snapshot.clone());
        let (shutdown, stopped) = oneshot::channel();
        let (events, event_rx) = std::sync::mpsc::sync_channel(64);
        let emit = Emitter {
            events,
            ctx: eframe::egui::Context::default(),
        };
        let expected = media.clone();
        let task = tokio::spawn(async move {
            // No FFmpeg path exists or is required for this mode.
            super::super::host::run(
                endpoint,
                String::new(),
                None,
                Some(media),
                rx,
                stopped,
                emit,
            )
            .await
        });
        let (tcp, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
        async fn receive(socket: &mut WebSocketStream<TcpStream>) -> Message {
            let incoming = tokio::time::timeout(Duration::from_secs(3), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            serde_json::from_str(incoming.to_text().unwrap()).unwrap()
        }
        async fn send(socket: &mut WebSocketStream<TcpStream>, message: Message) {
            socket
                .send(WsMessage::Text(
                    serde_json::to_string(&message).unwrap().into(),
                ))
                .await
                .unwrap();
        }
        assert!(
            matches!(receive(&mut socket).await, Message::Create { media: Some(actual), .. } if actual == expected)
        );
        send(
            &mut socket,
            Message::Created {
                url: "https://watch.example/watch/test#secret".into(),
                ice_servers: vec![],
            },
        )
        .await;
        assert!(
            matches!(receive(&mut socket).await, Message::State { state } if state == snapshot.state)
        );
        send(
            &mut socket,
            Message::PeerJoined {
                peer: "viewer".into(),
            },
        )
        .await;
        send(
            &mut socket,
            Message::Control {
                control: Control::Seek { position: 30.0 },
            },
        )
        .await;
        let mut events = vec![];
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                events.extend(event_rx.try_iter());
                if events.iter().any(|e| matches!(e, Event::Control(Control::Seek { position }) if *position == 30.0)) { break; }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        let mut updated = snapshot;
        updated.access.seek = false;
        snapshots.send(updated).unwrap();
        loop {
            if let Message::Access { access } = receive(&mut socket).await {
                assert!(!access.seek);
                break;
            }
        }
        shutdown.send(()).unwrap();
        loop {
            if matches!(receive(&mut socket).await, Message::Ended) {
                break;
            }
        }
        task.await.unwrap().unwrap();
        events.extend(event_rx.try_iter());
        assert!(events.iter().any(|e| matches!(e, Event::Created(_))));
        assert!(events.iter().any(|e| matches!(e, Event::Viewers(1))));
        assert!(events.iter().all(|e| !matches!(e, Event::Error(_))));
    }
}
