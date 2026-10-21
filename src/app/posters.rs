//! Small, visible-only poster requests; downloading and decoding stay off UI.
use eframe::egui;
use std::{collections::VecDeque, io::Cursor, sync::mpsc, time::Duration};

struct Entry {
    url: String,
    pending: bool,
    texture: Option<egui::TextureHandle>,
}
struct Download {
    url: String,
    image: Option<egui::ColorImage>,
}
pub(super) struct Posters {
    entries: VecDeque<Entry>,
    sender: mpsc::Sender<Download>,
    receiver: mpsc::Receiver<Download>,
}
impl Default for Posters {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            entries: VecDeque::new(),
            sender,
            receiver,
        }
    }
}
impl Posters {
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(download) = self.receiver.try_recv() {
            if let Some(entry) = self.entries.iter_mut().find(|e| e.url == download.url) {
                entry.pending = false;
                entry.texture = download.image.map(|image| {
                    ctx.load_texture(&download.url, image, egui::TextureOptions::LINEAR)
                });
            }
        }
    }
    pub fn get(&mut self, url: &str, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if let Some(index) = self.entries.iter().position(|e| e.url == url) {
            let entry = self.entries.remove(index).unwrap();
            let texture = entry.texture.clone();
            self.entries.push_back(entry);
            return texture;
        }
        if self.entries.iter().filter(|e| e.pending).count() >= 4 {
            return None;
        }
        if self.entries.len() >= 64 {
            let index = self.entries.iter().position(|e| !e.pending)?;
            self.entries.remove(index);
        }
        self.entries.push_back(Entry {
            url: url.into(),
            pending: true,
            texture: None,
        });
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        let url = url.to_owned();
        ehttp::fetch(
            ehttp::Request::get(&url).with_timeout(Some(Duration::from_secs(10))),
            move |result| {
                let image = result
                    .ok()
                    .filter(|r| r.ok && r.bytes.len() <= 1024 * 1024)
                    .and_then(|r| decode(&r.bytes));
                let _ = sender.send(Download { url, image });
                ctx.request_repaint();
            },
        );
        None
    }
}
fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(512);
    limits.max_image_height = Some(768);
    limits.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(limits);
    let rgba = reader.decode().ok()?.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires Deno, Cinejoy availability and the TMDB poster CDN"]
    fn real_catalog_poster_downloads_and_decodes() {
        let mut search = crate::streaming::movie_search::Search::start("Gone Girl").unwrap();
        let movies = loop {
            if let Some(result) = search.poll() {
                let update = result.unwrap();
                if update.movies.iter().any(|m| m.id == 210577) || update.done {
                    break update.movies;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let url = movies
            .iter()
            .find(|m| m.id == 210577)
            .unwrap()
            .poster_url()
            .expect("catalog poster");
        let ctx = egui::Context::default();
        let mut posters = Posters::default();
        posters.get(&url, &ctx);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            posters.poll(&ctx);
            if let Some(texture) = posters.get(&url, &ctx) {
                assert_eq!(texture.size()[0], 154);
                assert!(texture.size()[1] > texture.size()[0]);
                assert_eq!(posters.entries.len(), 1);
                break;
            }
            assert!(
                posters.entries[0].pending,
                "poster download or decoding failed"
            );
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
