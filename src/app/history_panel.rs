use crate::history::{self, History};
use eframe::egui;
use std::time::{Duration, Instant};

pub(super) struct WatchHistory {
    pub store: History,
    artwork: super::history_artwork::HistoryArtwork,
    pub movie_posters: std::collections::HashMap<String, String>,
    session: Option<Session>,
    resume: Option<Resume>,
    checkpoint: Instant,
    pub error: Option<String>,
}
struct Session {
    source: String,
    title: String,
}
struct Resume {
    generation: u64,
    position: f64,
    deadline: Option<Instant>,
}
impl Default for WatchHistory {
    fn default() -> Self {
        #[cfg(not(test))]
        let loaded = History::load();
        #[cfg(test)]
        let loaded: Result<History, String> = Ok(History::memory());
        let (store, error) = match loaded {
            Ok(store) => (store, None),
            Err(error) => (History::empty(), Some(error)),
        };
        Self {
            store,
            artwork: Default::default(),
            movie_posters: Default::default(),
            session: None,
            resume: None,
            checkpoint: Instant::now(),
            error,
        }
    }
}
impl WatchHistory {
    pub fn flush(&mut self) {
        if let Err(error) = self.store.flush() {
            self.error = Some(error);
        }
    }
    pub fn cancel_session(&mut self) {
        self.session = None;
        self.resume = None;
    }
}
impl super::PlayerApp {
    pub(super) fn open_history(&mut self) {
        self.show_playlist = true;
        self.sidebar_tab = super::SidebarTab::History;
    }
    pub(super) fn checkpoint_history(&mut self) {
        let Some(session) = &self.watch_history.session else {
            return;
        };
        let Some(player) = &self.player else {
            return;
        };
        if self.loading || player.state.idle || self.watch_history.resume.is_some() {
            return;
        }
        let state = &player.state;
        let title = if session.title.contains("://") && !state.title.is_empty() {
            &state.title
        } else {
            &session.title
        };
        self.watch_history.store.record(
            &session.source,
            title,
            if state.seekable {
                self.pending_seek
                    .map_or(state.position, |(target, _)| target)
            } else {
                0.0
            },
            state.duration,
            false,
        );
        if let Some(url) = self.watch_history.movie_posters.get(&session.source) {
            self.watch_history
                .store
                .set_poster(&session.source, url.clone());
        }
    }
    pub(super) fn history_loaded(&mut self) {
        if let Some(media) = self.current.and_then(|i| self.queue.get(i)) {
            self.watch_history.session = Some(Session {
                source: history::source_identity(&media.source),
                title: media.name.clone(),
            });
            self.watch_history.checkpoint = Instant::now();
            if let Some(resume) = &mut self.watch_history.resume {
                resume.deadline = Some(Instant::now() + Duration::from_secs(10));
            }
        }
    }
    pub(super) fn history_finished(&mut self) {
        if let Some(session) = self.watch_history.session.take() {
            let old = self.watch_history.store.entry(&session.source);
            let title = old
                .map_or(session.title.as_str(), |e| e.title.as_str())
                .to_owned();
            let duration = old.map_or_else(
                || self.player.as_ref().map_or(0.0, |p| p.state.duration),
                |e| e.duration,
            );
            self.watch_history
                .store
                .record(&session.source, &title, 0.0, duration, true);
            if let Some(url) = self.watch_history.movie_posters.get(&session.source) {
                self.watch_history
                    .store
                    .set_poster(&session.source, url.clone());
            }
            self.watch_history.flush();
        }
        self.watch_history.resume = None;
    }
    pub(super) fn poll_history(&mut self) {
        if let Some(resume) = &self.watch_history.resume {
            if resume.generation != self.playback_generation {
                self.watch_history.resume = None;
            } else if !self.loading
                && let Some(player) = &self.player
            {
                if player.state.seekable {
                    let position = if player.state.duration > 0.0 {
                        resume.position.min((player.state.duration - 0.1).max(0.0))
                    } else {
                        resume.position
                    };
                    self.watch_history.resume = None;
                    self.commit_seek(position);
                } else if resume
                    .deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    self.watch_history.resume = None;
                    self.watch_history.error = Some("This video cannot seek to the saved position; playback will continue from its available start.".into());
                }
            }
        }
        if self.watch_history.checkpoint.elapsed() >= Duration::from_secs(1) {
            self.checkpoint_history();
            self.watch_history.checkpoint = Instant::now();
        }
        if let Err(error) = self.watch_history.store.flush_if_due() {
            self.watch_history.error = Some(error);
        }
    }
    pub(super) fn play_from_history(&mut self, source: &str) {
        self.watch_history.error = None;
        self.checkpoint_history();
        self.watch_history.flush();
        let Some(entry) = self.watch_history.store.entry(source).cloned() else {
            return;
        };
        if !entry.source.contains("://") && !std::path::Path::new(&entry.source).is_file() {
            self.watch_history.error = Some(format!(
                "{} is unavailable. The video may have been moved or deleted.",
                entry.title
            ));
            return;
        }
        let index = self
            .queue
            .iter()
            .position(|m| history::source_identity(&m.source) == entry.source)
            .unwrap_or_else(|| {
                self.queue.push(super::Media {
                    source: entry.source.clone(),
                    name: entry.title.clone(),
                });
                self.queue.len() - 1
            });
        self.queue[index].name = entry.title;
        let generation = self.playback_generation;
        self.play(index);
        if self.playback_generation != generation && entry.position > 0.0 && !entry.completed {
            self.watch_history.resume = Some(Resume {
                generation: self.playback_generation,
                position: entry.position,
                deadline: None,
            });
        }
    }
    pub(super) fn history_panel(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        let mut remove = None;
        let mut clear = false;
        self.watch_history
            .artwork
            .poll(ui.ctx(), &mut self.watch_history.store);
        ui.small("Recently watched · choose an item to continue");
        if let Some(error) = &self.watch_history.error {
            ui.colored_label(super::ACCENT, error);
        }
        let has_history = !self.watch_history.store.entries().is_empty();
        let footer_height = if has_history {
            ui.spacing().interact_size.y + ui.spacing().item_spacing.y * 2.0 + 1.0
        } else {
            0.0
        };
        egui::ScrollArea::vertical()
            .id_salt("watch_history")
            .auto_shrink([false, false])
            .max_height(
                (ui.available_height() - footer_height - if self.fullscreen { 34.0 } else { 0.0 })
                    .max(0.0),
            )
            .show(ui, |ui| {
                if self.watch_history.store.entries().is_empty() {
                    ui.label("No watch history yet. Play a movie or video to see it here.");
                }
                for (movies, heading, empty) in [
                    (true, "Movies", "No movies watched yet."),
                    (false, "Videos", "No videos watched yet."),
                ] {
                    let entries: Vec<_> = self
                        .watch_history
                        .store
                        .entries()
                        .iter()
                        .filter(|entry| entry.is_movie() == movies)
                        .collect();
                    ui.add_space(8.0);
                    egui::CollapsingHeader::new(
                        egui::RichText::new(format!("{heading} · {}", entries.len())).strong(),
                    )
                    .id_salt(("history_section", movies))
                    .default_open(true)
                    .show_unindented(ui, |ui| {
                        if entries.is_empty() {
                            ui.weak(empty);
                        }
                        for entry in entries {
                            ui.push_id(&entry.source, |ui| {
                                ui.horizontal(|ui| {
                                    let position = super::format_time(entry.position);
                                    let status = if entry.completed {
                                        "Watched · Play again".into()
                                    } else if entry.position > 0.0 {
                                        if entry.duration > 0.0 {
                                            format!(
                                                "Resume at {position} / {}",
                                                super::format_time(entry.duration)
                                            )
                                        } else {
                                            format!("Resume at {position}")
                                        }
                                    } else {
                                        "Play from start".into()
                                    };
                                    let width = (ui.available_width() - 28.0).max(50.0);
                                    let height = if movies { 72.0 } else { 60.0 };
                                    let response = ui
                                        .add_enabled_ui(self.player.is_some(), |ui| {
                                            ui.allocate_response(
                                                egui::vec2(width, height),
                                                egui::Sense::click(),
                                            )
                                        })
                                        .inner;
                                    response.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            self.player.is_some(),
                                            format!("Resume {} at {position}", entry.title),
                                        )
                                    });
                                    if ui.is_rect_visible(response.rect) {
                                        let rect = response.rect;
                                        let visuals = ui.style().interact(&response);
                                        ui.painter().rect_filled(rect, 5.0, visuals.bg_fill);
                                        let size =
                                            if crate::streaming::cinejoy::movie_id(&entry.source)
                                                .is_some()
                                            {
                                                egui::vec2(40.0, 60.0)
                                            } else {
                                                egui::vec2(72.0, 40.5)
                                            };
                                        let artwork = egui::Rect::from_min_size(
                                            egui::pos2(
                                                rect.left() + 6.0,
                                                rect.center().y - size.y / 2.0,
                                            ),
                                            size,
                                        );
                                        if let Some(texture) =
                                            self.watch_history.artwork.get(entry, ui.ctx())
                                        {
                                            egui::Image::new((texture.id(), texture.size_vec2()))
                                                .fit_to_exact_size(size)
                                                .corner_radius(3.0)
                                                .paint_at(ui, artwork);
                                        } else {
                                            ui.painter().rect_filled(
                                                artwork,
                                                3.0,
                                                egui::Color32::from_gray(35),
                                            );
                                            ui.painter().text(
                                                artwork.center(),
                                                egui::Align2::CENTER_CENTER,
                                                "▶",
                                                egui::FontId::proportional(16.0),
                                                egui::Color32::from_gray(140),
                                            );
                                        }
                                        let left = artwork.right() + 8.0;
                                        let text_width = (rect.right() - left - 6.0).max(0.0);
                                        let title = egui::Rect::from_min_size(
                                            egui::pos2(left, rect.center().y - 20.0),
                                            egui::vec2(text_width, 20.0),
                                        );
                                        ui.place(
                                            title,
                                            egui::Label::new(
                                                egui::RichText::new(&entry.title).strong(),
                                            )
                                            .truncate()
                                            .halign(egui::Align::Min)
                                            .selectable(false),
                                        );
                                        let subtitle = title.translate(egui::vec2(0.0, 22.0));
                                        ui.place(
                                            subtitle,
                                            egui::Label::new(
                                                egui::RichText::new(status).small().weak(),
                                            )
                                            .truncate()
                                            .halign(egui::Align::Min)
                                            .selectable(false),
                                        );
                                        if entry.duration > 0.0 {
                                            let progress = if entry.completed {
                                                1.0
                                            } else {
                                                (entry.position / entry.duration).clamp(0.0, 1.0)
                                                    as f32
                                            };
                                            let bar = egui::Rect::from_min_size(
                                                egui::pos2(left, rect.bottom() - 5.0),
                                                egui::vec2(text_width * progress, 2.0),
                                            );
                                            ui.painter().rect_filled(bar, 1.0, super::ACCENT);
                                        }
                                    }
                                    if response.on_hover_text(&entry.source).clicked() {
                                        chosen = Some(entry.source.clone());
                                    }
                                    if super::icons::small_button(
                                        ui,
                                        super::Icon::Close,
                                        "Remove from history",
                                    )
                                    .clicked()
                                    {
                                        remove = Some(entry.source.clone());
                                    }
                                });
                                ui.add_space(4.0);
                            });
                        }
                    });
                }
            });
        if has_history {
            ui.separator();
            clear = ui.button("Clear history").clicked();
        }
        if clear {
            self.watch_history.store.clear();
            self.watch_history.session = None;
            self.watch_history.flush();
        } else if let Some(source) = remove {
            self.watch_history.store.remove(&source);
            if self
                .watch_history
                .session
                .as_ref()
                .is_some_and(|s| s.source == source)
            {
                self.watch_history.session = None;
            }
            self.watch_history.flush();
        } else if let Some(source) = chosen {
            self.play_from_history(&source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::PlayerApp, playback::Player, subtitles::library::tests::Fixture};
    fn frame(
        app: &mut PlayerApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| app.ui(ctx),
        )
    }
    fn named_button(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        let bounds = output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find_map(|(_, n)| (n.label() == Some(label)).then(|| n.bounds()).flatten())
            .unwrap_or_else(|| panic!("Missing button {label}"));
        egui::pos2(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        )
    }
    fn click(app: &mut PlayerApp, ctx: &egui::Context, pos: egui::Pos2) {
        frame(
            app,
            ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }
    fn wait(app: &mut PlayerApp, ctx: &egui::Context, check: impl Fn(&PlayerApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            frame(app, ctx, vec![]);
            assert!(app.error.is_none(), "{:?}", app.error);
            if check(app) {
                return;
            }
            assert!(Instant::now() < deadline, "History playback timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn history_tab_renders_empty_state_and_missing_file_stays_in_history() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.open_history();
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        assert!(
            output
                .platform_output
                .accesskit_update
                .unwrap()
                .nodes
                .iter()
                .any(|(_, n)| n
                    .label()
                    .or(n.value())
                    .is_some_and(|s| s.starts_with("No watch history yet")))
        );
        app.watch_history.store.record(
            "/missing/nvplay-movie.mp4",
            "Missing movie",
            25.0,
            60.0,
            false,
        );
        app.play_from_history("/missing/nvplay-movie.mp4");
        assert!(
            app.watch_history
                .error
                .as_deref()
                .unwrap()
                .contains("unavailable")
        );
        assert_eq!(app.watch_history.store.entries().len(), 1);
        assert!(app.queue.is_empty());
    }
    #[test]
    fn history_groups_catalog_and_network_movies_separately_from_local_videos() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.watch_history.store.record(
            "https://cinejoy.pk/watch/movie/210577",
            "Gone Girl",
            20.0,
            6000.0,
            false,
        );
        app.watch_history.store.record(
            "https://example.com/stream.m3u8",
            "Network movie",
            12.0,
            90.0,
            false,
        );
        app.watch_history
            .store
            .record("/missing/clip.mp4", "My clip", 5.0, 30.0, false);
        app.open_history();
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        let update = output.platform_output.accesskit_update.unwrap();
        for label in ["Movies · 2", "Videos · 1"] {
            assert!(
                update
                    .nodes
                    .iter()
                    .any(|(_, n)| n.label().or(n.value()) == Some(label)),
                "Missing {label}"
            );
        }
    }
    #[test]
    fn clear_history_does_not_reinsert_current_session() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.watch_history
            .store
            .record("movie", "Title", 20.0, 60.0, false);
        app.watch_history.session = Some(Session {
            source: "movie".into(),
            title: "Title".into(),
        });
        app.open_history();
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, named_button(&output, "Clear history"));
        assert!(app.watch_history.store.entries().is_empty());
        assert!(app.watch_history.session.is_none());
    }
    #[test]
    #[ignore = "requires libmpv and NVPLAY_TEST_MEDIA; verifies saved history through sidebar click and real resume after restart"]
    fn real_history_resumes_from_saved_timestamp_after_restart() {
        let fixture = Fixture::new();
        let path = fixture.0.join("history.json");
        let source = std::env::var("NVPLAY_TEST_MEDIA").expect("NVPLAY_TEST_MEDIA");
        let identity = history::source_identity(&source);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(
            &ctx,
            Some(Player::for_ui_test(&ctx).unwrap()),
            None,
            None,
            vec![],
        );
        app.watch_history.store = History::at(path.clone()).unwrap();
        app.queue.push(super::super::Media {
            source,
            name: "History test movie".into(),
        });
        app.play(0);
        wait(&mut app, &ctx, |app| {
            !app.loading && app.player.as_ref().unwrap().state.seekable
        });
        app.command(&["set", "pause", "yes"]);
        app.commit_seek(8.5);
        wait(&mut app, &ctx, |app| {
            app.pending_seek.is_none()
                && (app.player.as_ref().unwrap().state.position - 8.5).abs() < 0.2
        });
        app.stop();
        let entry = app.watch_history.store.entry(&identity).unwrap();
        assert!((entry.position - 8.5).abs() < 0.2);
        assert_eq!(entry.title, "History test movie");
        drop(app);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(
            &ctx,
            Some(Player::for_ui_test(&ctx).unwrap()),
            None,
            None,
            vec![],
        );
        app.watch_history.store = History::at(path.clone()).unwrap();
        app.open_history();
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        let label = format!(
            "Resume History test movie at {}",
            super::super::format_time(app.watch_history.store.entry(&identity).unwrap().position)
        );
        click(&mut app, &ctx, named_button(&output, &label));
        wait(&mut app, &ctx, |app| {
            !app.loading
                && app.pending_seek.is_none()
                && app.player.as_ref().unwrap().state.position >= 8.3
        });
        assert_eq!(app.queue.len(), 1);
        assert!(
            app.watch_history.error.is_none(),
            "{:?}",
            app.watch_history.error
        );
        assert!(app.player.as_ref().unwrap().state.position < 12.0);
        app.stop();
        let saved = History::at(path).unwrap();
        assert!(saved.entry(&identity).unwrap().position >= 8.3);
    }
}
