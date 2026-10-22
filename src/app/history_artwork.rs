//! Bounded history artwork work; neither network nor frame decoding blocks UI.
use crate::history::{Entry, History};
use eframe::egui;
use std::{collections::HashMap, path::PathBuf, sync::mpsc, time::Duration};

enum Artwork {
    Frame(String, Option<egui::ColorImage>),
    Poster(String, Option<String>),
}
#[derive(Default)]
struct Frame {
    pending: bool,
    texture: Option<egui::TextureHandle>,
}
pub(super) struct HistoryArtwork {
    frames: HashMap<String, Frame>,
    movies: HashMap<String, bool>,
    posters: super::posters::Posters,
    sender: mpsc::Sender<Artwork>,
    receiver: mpsc::Receiver<Artwork>,
    ffmpeg: Option<PathBuf>,
}
impl Default for HistoryArtwork {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            frames: HashMap::new(),
            movies: HashMap::new(),
            posters: Default::default(),
            sender,
            receiver,
            ffmpeg: crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg"),
        }
    }
}
impl HistoryArtwork {
    pub fn poll(&mut self, ctx: &egui::Context, history: &mut History) {
        self.posters.poll(ctx);
        while let Ok(artwork) = self.receiver.try_recv() {
            match artwork {
                Artwork::Frame(source, image) => {
                    if let Some(frame) = self.frames.get_mut(&source) {
                        frame.pending = false;
                        frame.texture = image.map(|image| {
                            ctx.load_texture(
                                format!("history:{source}"),
                                image,
                                egui::TextureOptions::LINEAR,
                            )
                        });
                    }
                }
                Artwork::Poster(source, url) => {
                    if let Some(pending) = self.movies.get_mut(&source) {
                        *pending = false;
                    }
                    if let Some(url) = url {
                        history.set_poster(&source, url);
                    }
                }
            }
        }
        // History is bounded; release artwork for removed entries.
        self.frames
            .retain(|source, _| history.entry(source).is_some());
        self.movies
            .retain(|source, _| history.entry(source).is_some());
    }
    pub fn get(&mut self, entry: &Entry, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let catalog = crate::streaming::cinejoy::movie_id(&entry.source);
        if let Some(url) = &entry.poster_url {
            if let Some(texture) = self.posters.get(url, ctx) {
                return Some(texture);
            }
            if catalog.is_some() {
                return None;
            }
        }
        if let Some(id) = catalog {
            if self.movies.contains_key(&entry.source)
                || self.movies.values().filter(|p| **p).count() >= 2
            {
                return None;
            }
            self.movies.insert(entry.source.clone(), true);
            let source = entry.source.clone();
            let sender = self.sender.clone();
            let ctx = ctx.clone();
            // Same public catalog integration used by movie search. Resolve by ID
            // so older history and directly opened movie links get the right poster.
            let url = format!(
                "https://api.themoviedb.org/3/movie/{id}?api_key=8476a7ab80ad76f0936744df0430e67c"
            );
            ehttp::fetch(
                ehttp::Request::get(url).with_timeout(Some(Duration::from_secs(10))),
                move |result| {
                    let url = result
                        .ok()
                        .filter(|r| r.ok && r.bytes.len() <= 1024 * 1024)
                        .and_then(|r| serde_json::from_slice::<serde_json::Value>(&r.bytes).ok())
                        .and_then(|v| {
                            crate::streaming::movie_search::Movie {
                                id: 0,
                                title: String::new(),
                                year: String::new(),
                                poster_path: v["poster_path"].as_str().map(str::to_owned),
                            }
                            .poster_url()
                        });
                    let _ = sender.send(Artwork::Poster(source, url));
                    ctx.request_repaint();
                },
            );
            return None;
        }
        if let Some(frame) = self.frames.get(&entry.source) {
            return frame.texture.clone();
        }
        if self.frames.values().filter(|f| f.pending).count() >= 2 {
            return None;
        }
        let ffmpeg = self.ffmpeg.clone();
        if ffmpeg.is_none() && !crate::streaming::is_webpage(&entry.source) {
            return None;
        }
        self.frames.insert(
            entry.source.clone(),
            Frame {
                pending: true,
                texture: None,
            },
        );
        let source = entry.source.clone();
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (thumbnail, playable) = network_artwork(&source);
            if let Some(thumbnail) = thumbnail {
                let _ = sender.send(Artwork::Poster(source.clone(), Some(thumbnail)));
                ctx.request_repaint();
            }
            // Also retain a first-frame fallback if a site's thumbnail expires
            // or its format cannot be decoded. Resolution stays thumbnail-sized.
            let image = ffmpeg.and_then(|ffmpeg| {
                super::preview::first_frame(&ffmpeg, playable.as_deref().unwrap_or(&source))
            });
            let _ = sender.send(Artwork::Frame(source, image));
            ctx.request_repaint();
        });
        None
    }
}

/// Resolve only website pages. Direct streams go straight to the bounded frame
/// decoder; yt-dlp handles site metadata without downloading the whole video.
fn network_artwork(source: &str) -> (Option<String>, Option<String>) {
    use std::{io::Read, process::Stdio, time::Instant};
    if !crate::streaming::is_webpage(source) {
        return (None, None);
    }
    let Some(tool) = crate::streaming::find_tool("NVPLAY_YTDLP", "yt-dlp") else {
        return (None, None);
    };
    let mut command = crate::streaming::tool_command(&tool);
    command
        .args([
            "--no-playlist",
            "--playlist-items",
            "1",
            "--no-warnings",
            "--skip-download",
            "--socket-timeout",
            "8",
            "--retries",
            "0",
            "--format",
            "best[height<=360]/worstvideo/worst",
            "--print",
            "%(thumbnail)s",
            "--print",
            "%(url)s",
            "--",
            source,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return (None, None);
    };
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(16 * 1024 + 1).read_to_end(&mut bytes).ok()?;
        (bytes.len() <= 16 * 1024).then_some(bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let bytes = reader.join().ok().flatten();
    if !success {
        return (None, None);
    }
    bytes
        .and_then(|b| String::from_utf8(b).ok())
        .map_or((None, None), |text| {
            let mut lines = text.lines();
            let thumbnail = lines.next().filter(|s| public_image(s)).map(str::to_owned);
            let playable = lines.next().filter(|s| public_image(s)).map(str::to_owned);
            (thumbnail, playable)
        })
}
fn public_image(value: &str) -> bool {
    url::Url::parse(value)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires yt-dlp, FFmpeg, and public website thumbnail access"]
    fn website_history_resolves_and_downloads_its_thumbnail() {
        let source = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
        let ctx = egui::Context::default();
        let mut artwork = HistoryArtwork::default();
        let mut history = History::memory();
        history.record(source, "Website video", 1.0, 10.0, false);
        assert!(history.entry(source).unwrap().is_movie());
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            artwork.poll(&ctx, &mut history);
            if let Some(texture) = artwork.get(history.entry(source).unwrap(), &ctx) {
                assert!(texture.size()[0] > 0);
                assert!(history.entry(source).unwrap().poster_url.is_some());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Website thumbnail did not load"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    #[ignore = "requires FFmpeg; generates a two-color video and checks history uses its first frame"]
    fn history_thumbnail_decodes_first_frame_and_reuses_texture() {
        let fixture = crate::subtitles::library::tests::Fixture::new();
        let source = fixture.0.join("red-then-blue.mp4");
        let ffmpeg = crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg").unwrap();
        assert!(
            std::process::Command::new(ffmpeg)
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=red:s=160x90:d=1",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=blue:s=160x90:d=1",
                    "-filter_complex",
                    "[0:v][1:v]concat=n=2:v=1:a=0",
                    "-c:v",
                    "mpeg4",
                ])
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        let source = source.to_string_lossy().into_owned();
        let ctx = egui::Context::default();
        let mut artwork = HistoryArtwork::default();
        let mut history = History::memory();
        history.record(&source, "Two colors", 1.5, 2.0, false);
        assert!(artwork.get(history.entry(&source).unwrap(), &ctx).is_none());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let texture = loop {
            artwork.poll(&ctx, &mut history);
            if let Some(texture) = artwork.get(history.entry(&source).unwrap(), &ctx) {
                break texture;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(texture.size(), [160, 90]);
        assert_eq!(
            artwork
                .get(history.entry(&source).unwrap(), &ctx)
                .unwrap()
                .id(),
            texture.id()
        );
        let image =
            super::super::preview::first_frame(&artwork.ffmpeg.clone().unwrap(), &source).unwrap();
        let pixel = image.pixels[45 * 160 + 80];
        assert!(
            pixel.r() > 240 && pixel.b() < 15,
            "expected red first frame: {pixel:?}"
        );
        // The same frame must appear for direct HTTP streams in Movies.
        use std::io::{Read, Write};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let bytes = std::fs::read(&source).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let network = format!("http://{}/movie.mp4", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while !stopped.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
                if let Ok((mut socket, _)) = listener.accept() {
                    socket
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut request = [0; 4096];
                    let _ = socket.read(&mut request);
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    );
                    let _ = socket.write_all(headers.as_bytes());
                    let _ = socket.write_all(&bytes);
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });
        history.record(&network, "Network movie", 1.0, 2.0, false);
        assert!(history.entry(&network).unwrap().is_movie());
        artwork.get(history.entry(&network).unwrap(), &ctx);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            artwork.poll(&ctx, &mut history);
            if let Some(texture) = artwork.get(history.entry(&network).unwrap(), &ctx) {
                assert_eq!(texture.size(), [160, 90]);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "HTTP stream poster did not load"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        history.clear();
        artwork.poll(&ctx, &mut history);
        assert!(artwork.frames.is_empty());
    }
}
