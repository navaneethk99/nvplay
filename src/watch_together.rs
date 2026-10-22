mod azure;
mod host;
mod media;
mod video;

use std::{path::PathBuf, sync::mpsc};
use watch_protocol::{Control, Playback};

#[derive(Clone)]
pub struct Snapshot {
    pub source: String,
    pub audio_source: Option<String>,
    pub video_headers: String,
    pub audio_headers: String,
    pub video: bool,
    pub audio: bool,
    pub width: i64,
    pub height: i64,
    pub bitrate: u64,
    pub access: watch_protocol::Access,
    pub state: Playback,
}
pub enum Event {
    Created(String),
    Status(String),
    Viewers(usize),
    Control(Control),
    Error(String),
    Closed,
}
pub struct Host {
    snapshots: tokio::sync::watch::Sender<Snapshot>,
    events: mpsc::Receiver<Event>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}
impl Host {
    pub fn start(
        endpoint: &str,
        key: String,
        ffmpeg: PathBuf,
        snapshot: Snapshot,
        ctx: eframe::egui::Context,
    ) -> Result<Self, String> {
        let endpoint = signaling_url(endpoint)?;
        if !snapshot.video && !snapshot.audio {
            return Err("This media has no shareable audio or video track.".into());
        }
        let (snapshots, receiver) = tokio::sync::watch::channel(snapshot);
        let (events, event_rx) = mpsc::sync_channel(64);
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("nvplay-watch".into())
            .spawn(move || {
                let emit = Emitter { events, ctx };
                match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => {
                        if let Err(error) = runtime.block_on(host::run(
                            endpoint,
                            key,
                            ffmpeg,
                            receiver,
                            stopped,
                            emit.clone(),
                        )) {
                            emit.send(Event::Error(error));
                        }
                    }
                    Err(_) => {
                        emit.send(Event::Error("Could not start the sharing runtime.".into()))
                    }
                }
                emit.send(Event::Closed);
            })
            .map_err(|_| "Could not start the sharing worker")?;
        Ok(Self {
            snapshots,
            events: event_rx,
            shutdown: Some(shutdown),
        })
    }
    pub fn update(&self, snapshot: Snapshot) {
        self.snapshots.send_replace(snapshot);
    }
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
}
#[derive(Clone)]
struct Emitter {
    events: mpsc::SyncSender<Event>,
    ctx: eframe::egui::Context,
}
impl Emitter {
    fn send(&self, event: Event) {
        let _ = self.events.try_send(event);
        self.ctx.request_repaint();
    }
}
pub fn signaling_url(endpoint: &str) -> Result<String, String> {
    let mut url = url::Url::parse(endpoint.trim())
        .map_err(|_| "Enter the HTTPS address of your watch service.")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" if local => "ws",
        _ => {
            return Err(
                "The watch service must use HTTPS (HTTP is allowed on localhost for testing)."
                    .into(),
            );
        }
    };
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(
            "Enter only the watch service origin, such as https://watch.example.com.".into(),
        );
    }
    url.set_scheme(scheme).unwrap();
    url.set_path("/ws");
    Ok(url.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_signaling_requires_tls_and_does_not_accept_embedded_secrets() {
        assert_eq!(
            signaling_url("https://watch.example.com").unwrap(),
            "wss://watch.example.com/ws"
        );
        assert_eq!(
            signaling_url("http://localhost:8787").unwrap(),
            "ws://localhost:8787/ws"
        );
        for bad in [
            "http://example.com",
            "https://user:pass@example.com",
            "https://example.com/path",
            "https://example.com?key=secret",
        ] {
            assert!(signaling_url(bad).is_err());
        }
    }
}
