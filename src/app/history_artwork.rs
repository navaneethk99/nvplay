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
        if entry.is_movie() {
            if let Some(url) = &entry.poster_url {
                return self.posters.get(url, ctx);
            }
            if self.movies.contains_key(&entry.source)
                || self.movies.values().filter(|p| **p).count() >= 2
            {
                return None;
            }
            let id = crate::streaming::cinejoy::movie_id(&entry.source)?;
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
        let ffmpeg = self.ffmpeg.clone()?;
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
            let image = super::preview::first_frame(&ffmpeg, &source);
            let _ = sender.send(Artwork::Frame(source, image));
            ctx.request_repaint();
        });
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        history.clear();
        artwork.poll(&ctx, &mut history);
        assert!(artwork.frames.is_empty());
    }
}
