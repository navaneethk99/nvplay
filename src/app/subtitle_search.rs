use crate::subtitles::{self, Downloaded, Subtitle};
use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc::{self, Receiver};

enum Reply {
    Search(Vec<Subtitle>),
    Download(Downloaded),
}
pub(super) struct SubtitleSearch {
    pub open: bool,
    query: String,
    language: String,
    key: String,
    key_status: Option<String>,
    focus: bool,
    searched: bool,
    results: Vec<Subtitle>,
    pending: Option<Receiver<Result<Reply, String>>>,
    downloading: bool,
    error: Option<String>,
    target: u64,
}
impl Default for SubtitleSearch {
    fn default() -> Self {
        let saved_key = subtitles::settings::load();
        let settings_error = saved_key.as_ref().err().cloned();
        Self {
            open: false,
            query: String::new(),
            language: "en".into(),
            key: saved_key.unwrap_or_default(),
            key_status: settings_error,
            focus: false,
            searched: false,
            results: Vec::new(),
            pending: None,
            downloading: false,
            error: None,
            target: 0,
        }
    }
}
impl SubtitleSearch {
    pub fn open(&mut self, title: String, target: u64) {
        self.close();
        self.query = title;
        self.target = target;
        self.results.clear();
        self.error = None;
        self.searched = false;
        self.focus = true;
        self.open = true;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.pending = None;
    }
    fn start(&mut self, file_id: Option<u64>, ctx: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        let key = self.key.clone();
        let query = self.query.clone();
        let language = self.language.clone();
        let ctx = ctx.clone();
        self.error = None;
        self.downloading = file_id.is_some();
        self.pending = Some(receiver);
        if file_id.is_none() {
            self.results.clear();
            self.searched = false;
        }
        let result = std::thread::Builder::new()
            .name("subtitle-request".into())
            .spawn(move || {
                let result = match file_id {
                    Some(id) => subtitles::download(&key, id).map(Reply::Download),
                    None => subtitles::search(&key, &query, &language).map(Reply::Search),
                };
                let _ = sender.send(result);
                ctx.request_repaint();
            });
        if result.is_err() {
            self.pending = None;
            self.error = Some("Could not start the subtitle request. Please try again.".into());
        }
    }
    pub fn ui(&mut self, ctx: &egui::Context, target: u64, ready: bool) -> Option<Downloaded> {
        if !self.open {
            return None;
        }
        if target != self.target {
            self.close();
            return None;
        }
        let mut downloaded = None;
        let reply = self
            .pending
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(reply) => Some(reply),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "The subtitle request stopped unexpectedly. Please try again.".into(),
                )),
            });
        if let Some(reply) = reply {
            self.pending = None;
            match reply {
                Ok(Reply::Search(results)) => {
                    self.results = results;
                    self.searched = true;
                }
                Ok(Reply::Download(file)) if ready => downloaded = Some(file),
                Ok(Reply::Download(_)) => {
                    self.error =
                        Some("The video is no longer playing. Open it and try again.".into())
                }
                Err(error) => self.error = Some(error),
            }
        }
        if downloaded.is_some() {
            self.close();
            return downloaded;
        }
        let width = (ctx.content_rect().width() - 48.0).clamp(200.0, 530.0);
        let mut close = false;
        let mut search = false;
        let mut chosen = None;
        let response = egui::Modal::new(egui::Id::new("download_subtitle")).show(ctx, |ui| {
            ui.set_width(width);
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 64.0).max(120.0)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Download subtitle");
                    if ui.button("Close").clicked() { close = true; }
                });
                ui.label("Search OpenSubtitles by movie title, then choose a release.");
                let busy = self.pending.is_some();
                ui.add_enabled_ui(!busy, |ui| {
                    let input = ui.add(egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::new("subtitle_query")).hint_text("Movie title…").char_limit(160).desired_width(width));
                    if self.focus { input.request_focus(); self.focus = false; }
                    let mut changed = input.changed();
                    let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        ui.label("Language code");
                        changed |= ui.add(egui::TextEdit::singleline(&mut self.language)
                            .hint_text("en").char_limit(10).desired_width(55.0)).changed();
                        search = ui.add_enabled(!self.query.trim().is_empty() && !self.language.trim().is_empty(), egui::Button::new("Search")).clicked() || enter;
                    });
                    ui.small("Examples: en, es, fr, de, hi, pt-br");
                    if changed { self.results.clear(); self.searched = false; self.error = None; }
                    egui::CollapsingHeader::new("Connection settings").default_open(self.key.is_empty()).show(ui, |ui| {
                        ui.label("OpenSubtitles API key");
                        if ui.add(egui::TextEdit::singleline(&mut self.key).password(true).desired_width(width)).changed() {
                            self.key_status = None;
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!self.key.trim().is_empty(), egui::Button::new("Save key")).clicked() {
                                self.key_status = Some(match subtitles::settings::save(&self.key) {
                                    Ok(()) => "API key saved for future launches.".into(),
                                    Err(error) => error,
                                });
                            }
                            if ui.button("Remove saved key").clicked() {
                                self.key_status = Some(match subtitles::settings::remove() {
                                    Ok(()) => { self.key.clear(); "Saved API key removed.".into() }
                                    Err(error) => error,
                                });
                            }
                        });
                        if let Some(status) = &self.key_status { ui.label(status); }
                        ui.small("Save once to reuse the key after restarting. Stored in your local app settings. NVPLAY_OPENSUBTITLES_API_KEY overrides the saved key at launch.");
                        ui.hyperlink_to("Get an API key", "https://www.opensubtitles.com/consumers");
                    });
                });
                if busy { ui.horizontal(|ui| { ui.spinner(); ui.label(if self.downloading { "Downloading subtitle…" } else { "Searching OpenSubtitles…" }); }); }
                if let Some(error) = &self.error { ui.colored_label(Color32::from_rgb(240, 100, 85), error); }
                if self.searched && self.results.is_empty() { ui.label("No subtitles found. Try another title or language."); }
                if !ready { ui.label("Open a video and wait for it to finish loading to download subtitles."); }
                for subtitle in &self.results {
                    ui.separator();
                    ui.label(RichText::new(&subtitle.title).strong());
                    ui.label(&subtitle.release);
                    ui.horizontal_wrapped(|ui| {
                        ui.small(format!("{} · {} downloads{}", subtitle.language, subtitle.downloads,
                            if subtitle.hearing_impaired { " · Hearing impaired" } else { "" }));
                        if ui.add_enabled(ready && !busy, egui::Button::new("Download and use")).clicked() { chosen = Some(subtitle.file_id); }
                    });
                }
                ui.separator();
                ui.hyperlink_to("Subtitles provided by OpenSubtitles", "https://www.opensubtitles.com");
                ui.small("Downloads use your API allowance. Match the release for the best timing.");
            });
        });
        if response.should_close() || close {
            self.close();
        } else if let Some(id) = chosen {
            self.start(Some(id), ctx);
        } else if search && !self.query.trim().is_empty() && !self.language.trim().is_empty() {
            self.start(None, ctx);
        }
        None
    }
}

impl super::PlayerApp {
    pub(super) fn open_subtitle_search(&mut self) {
        self.show_url = false;
        self.show_help = false;
        self.movie_search.close();
        let title = self
            .current
            .and_then(|i| self.queue.get(i))
            .map(|m| {
                if m.name.contains("://") {
                    self.player
                        .as_ref()
                        .map(|p| p.state.title.clone())
                        .unwrap_or_default()
                } else if m.source.contains("://") {
                    m.name.clone()
                } else {
                    std::path::Path::new(&m.name)
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .replace(['.', '_'], " ")
                }
            })
            .unwrap_or_default();
        self.subtitle_search.open(title, self.playback_generation);
    }
    pub(super) fn subtitle_search_dialog(&mut self, ctx: &egui::Context) {
        let ready = !self.loading && self.player.as_ref().is_some_and(|p| !p.state.idle);
        if let Some(file) = self
            .subtitle_search
            .ui(ctx, self.playback_generation, ready)
        {
            self.add_subtitle(file.path.clone());
            // Retain until asynchronous attachment and persistence have completed.
            self.downloaded_subtitles.push(file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    const SRT: &[u8] = b"1\n00:00:00,000 --> 00:01:00,000\nDownloaded subtitle\n";

    fn deliver(search: &mut SubtitleSearch, reply: Reply) {
        let (sender, receiver) = mpsc::channel();
        sender.send(Ok(reply)).unwrap();
        search.pending = Some(receiver);
    }

    #[test]
    fn stale_download_cannot_attach_and_is_cleaned_up() {
        let ctx = egui::Context::default();
        let mut search = SubtitleSearch::default();
        search.open("Movie".into(), 7);
        let file = Downloaded::save(SRT).unwrap();
        let path = file.path.clone();
        deliver(&mut search, Reply::Download(file));
        assert!(search.ui(&ctx, 8, true).is_none());
        assert!(!search.open);
        assert!(!path.exists());
    }

    #[test]
    fn dialog_preserves_connection_and_cleans_canceled_download() {
        let mut search = SubtitleSearch {
            key: "session key".into(),
            ..Default::default()
        };
        search.open("Movie".into(), 7);
        let file = Downloaded::save(SRT).unwrap();
        let path = file.path.clone();
        deliver(&mut search, Reply::Download(file));
        search.close();
        assert!(!path.exists());
        search.open("Another movie".into(), 8);
        assert_eq!(search.key, "session key");
        assert!(search.pending.is_none());
        assert!(search.results.is_empty());
    }

    #[test]
    fn popup_renders_and_escape_keeps_fullscreen() {
        let ctx = egui::Context::default();
        let mut app = super::super::PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.fullscreen = true;
        app.open_subtitle_search();
        assert!(app.subtitle_search.open);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(440.0, 260.0),
                )),
                ..Default::default()
            },
            |ctx| app.subtitle_search_dialog(ctx),
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert!(!app.subtitle_search.open);
        assert!(app.fullscreen);
    }

    #[test]
    #[ignore = "requires libmpv and NVPLAY_TEST_MEDIA; verifies download completion through the UI into mpv"]
    fn real_download_completion_selects_and_shows_subtitle() {
        let ctx = egui::Context::default();
        let player = crate::playback::Player::for_ui_test(&ctx).unwrap();
        let media = std::env::var_os("NVPLAY_TEST_MEDIA").expect("NVPLAY_TEST_MEDIA");
        let mut app =
            super::super::PlayerApp::from_parts(&ctx, Some(player), None, None, vec![media]);
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.loading {
            for event in app.player.as_mut().unwrap().poll() {
                match event {
                    crate::playback::PlaybackEvent::Loaded => app.loading = false,
                    crate::playback::PlaybackEvent::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        app.command(&["set", "sub-visibility", "no"]);
        app.open_subtitle_search();
        let file = Downloaded::save(SRT).unwrap();
        let path = file.path.clone();
        deliver(&mut app.subtitle_search, Reply::Download(file));
        app.subtitle_search_dialog(&ctx);
        assert!(!app.subtitle_search.open);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(path.exists());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for event in app.player.as_mut().unwrap().poll() {
                app.handle_playback_event(event);
            }
            assert!(app.error.is_none(), "{:?}", app.error);
            let player = app.player.as_ref().unwrap();
            if player.state.subtitles_visible
                && player
                    .state
                    .tracks
                    .iter()
                    .any(|t| t.kind == "sub" && t.selected)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "downloaded subtitle was not selected"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        app.stop();
        drop(app);
        assert!(!path.exists());
    }
}
