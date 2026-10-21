use crate::streaming::movie_search::{Movie, Search};
use eframe::egui::{self, Color32, Key, RichText};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct MovieSearch {
    pub open: bool,
    query: String,
    results: Vec<Movie>,
    selected: usize,
    user_selected: bool,
    checked: usize,
    total: usize,
    focus: bool,
    edited: Option<Instant>,
    worker: Option<Search>,
    searched: bool,
    activate_when_ready: bool,
    error: Option<String>,
    credits_open: bool,
    logo: Option<egui::TextureHandle>,
    posters: super::posters::Posters,
}
impl MovieSearch {
    pub fn open(&mut self) {
        let posters = std::mem::take(&mut self.posters);
        *self = Self {
            posters,
            open: true,
            focus: true,
            ..Default::default()
        };
    }
    pub fn close(&mut self) {
        self.open = false;
        self.worker = None;
        self.edited = None;
        self.activate_when_ready = false;
    }
    fn changed(&mut self) {
        self.worker = None;
        self.results.clear();
        self.selected = 0;
        self.user_selected = false;
        self.checked = 0;
        self.total = 0;
        self.error = None;
        self.searched = false;
        self.activate_when_ready = false;
        self.edited = (!self.query.trim().is_empty()).then(Instant::now);
    }
    fn start(&mut self) {
        self.edited = None;
        self.error = None;
        match Search::start(self.query.trim()) {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => self.error = Some(error),
        }
    }
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Movie> {
        if !self.open {
            return None;
        }
        if let Some(worker) = &mut self.worker {
            if let Some(result) = worker.poll() {
                match result {
                    Ok(update) => {
                        let selected = self
                            .user_selected
                            .then(|| self.results.get(self.selected).map(|m| m.id))
                            .flatten();
                        self.results = update.movies;
                        self.selected = selected
                            .and_then(|id| self.results.iter().position(|m| m.id == id))
                            .unwrap_or(0);
                        self.checked = update.checked;
                        self.total = update.total;
                        if update.done {
                            self.worker = None;
                            self.searched = true;
                            if update.checked < update.total {
                                self.error = Some("Some Cinejoy availability checks timed out. Only confirmed matches are shown; try again to recheck.".into());
                            }
                        }
                    }
                    Err(error) => {
                        self.worker = None;
                        self.error = Some(error);
                    }
                }
            }
            if self.worker.is_some() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        self.posters.poll(ctx);
        let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
        let down = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
        let up = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
        if (down || up) && !self.results.is_empty() {
            self.user_selected = true;
        }
        if down {
            self.selected = (self.selected + 1).min(self.results.len().saturating_sub(1));
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        let width = (ctx.content_rect().width() - 48.0).clamp(200.0, 500.0);
        let mut chosen = None;
        let mut close = false;
        let response = egui::Modal::new(egui::Id::new("movie_search"))
            .frame(
                egui::Frame::popup(&ctx.style())
                    .inner_margin(18.0)
                    .corner_radius(12.0),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Search movies").size(20.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(10.0);
                let input = ui.add_sized(
                    [width, 34.0],
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::new("movie_search_query"))
                        .font(egui::TextStyle::Heading)
                        .char_limit(160)
                        .hint_text("Movie title…"),
                );
                if self.focus {
                    input.request_focus();
                    self.focus = false;
                }
                if input.changed() {
                    self.changed();
                }
                if enter && !self.query.trim().is_empty() && (self.edited.is_some() || self.worker.is_some() || self.error.is_some()) {
                    self.activate_when_ready = true;
                    if self.worker.is_none() { self.start(); }
                }
                if let Some(edited) = self.edited {
                    let wait = Duration::from_millis(350).saturating_sub(edited.elapsed());
                    if wait.is_zero() {
                        self.start();
                    } else {
                        ctx.request_repaint_after(wait);
                    }
                }
                ui.add_space(8.0);
                let checking = self.worker.is_some() || self.edited.is_some();
                if checking && self.results.is_empty() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(if self.total == 0 { "Searching Cinejoy…".into() } else { format!("Checking Cinejoy availability · {} / {}", self.checked, self.total) });
                    });
                }
                if let Some(error) = self.error.clone() {
                    ui.label(RichText::new(error).color(Color32::from_rgb(240, 100, 85)));
                    if ui.button("Try again").clicked() { self.start(); }
                }
                if self.results.is_empty() {
                    if !checking && self.error.is_none() {
                        ui.label(if self.searched { "No playable Cinejoy matches found. Try another title." } else { "Find a movie available on Cinejoy." });
                    }
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("movie_results")
                        .max_height((ctx.content_rect().height() - if self.credits_open { 285.0 } else { 225.0 }).clamp(68.0, 300.0))
                        .show(ui, |ui| {
                            for (index, movie) in self.results.iter().enumerate() {
                                let (rect, row) = ui.allocate_exact_size(egui::vec2(width, 66.0), egui::Sense::click());
                                row.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), movie.label()));
                                if ui.is_rect_visible(rect) {
                                    let visuals = ui.style().interact_selectable(&row, index == self.selected);
                                    ui.painter().rect(rect, 6.0, visuals.weak_bg_fill, visuals.bg_stroke, egui::StrokeKind::Inside);
                                    let poster = egui::Rect::from_min_size(rect.min + egui::vec2(8.0, 6.0), egui::vec2(36.0, 54.0));
                                    if let Some(texture) = movie.poster_url().and_then(|url| self.posters.get(&url, ctx)) {
                                        egui::Image::new((texture.id(), texture.size_vec2())).fit_to_exact_size(poster.size()).corner_radius(3.0).paint_at(ui, poster);
                                    } else {
                                        ui.painter().rect_filled(poster, 3.0, Color32::from_gray(40));
                                        ui.painter().text(poster.center(), egui::Align2::CENTER_CENTER, "—", egui::FontId::proportional(16.0), Color32::from_gray(130));
                                    }
                                    let title = egui::Rect::from_min_max(rect.min + egui::vec2(56.0, 13.0), egui::pos2(rect.right() - 10.0, rect.top() + 35.0));
                                    ui.place(title, egui::Label::new(RichText::new(&movie.title).strong().color(visuals.text_color())).truncate().halign(egui::Align::Min).selectable(false));
                                    let year = egui::Rect::from_min_max(title.min + egui::vec2(0.0, 24.0), egui::pos2(title.right(), rect.bottom() - 9.0));
                                    ui.place(year, egui::Label::new(RichText::new(if movie.year.is_empty() { "Year unknown" } else { &movie.year }).small().weak()).truncate().halign(egui::Align::Min).selectable(false));
                                }
                                if (down || up) && index == self.selected {
                                    row.scroll_to_me(Some(egui::Align::Center));
                                }
                                if row.clicked() || ((enter || self.activate_when_ready) && index == self.selected) {
                                    chosen = Some(movie.clone());
                                }
                            }
                        });
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new("↑ ↓ to choose · Enter to play · Esc to close")
                        .small()
                        .weak(),
                );
                ui.horizontal(|ui| {
                    if checking && !self.results.is_empty() {
                        ui.add(egui::Spinner::new().size(10.0));
                        ui.label(RichText::new(format!("Checking Cinejoy availability · {} / {}", self.checked, self.total)).small().weak());
                    } else { ui.label(RichText::new("Available on Cinejoy").small().weak()); }
                });
                let credits = ui.collapsing("Movie data credits", |ui| {
                    if self.logo.is_none() && let Ok(image) = image::load_from_memory(include_bytes!("assets/tmdb.png")) {
                        let rgba = image.to_rgba8();
                        self.logo = Some(ctx.load_texture("tmdb_logo", egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize], rgba.as_raw()), Default::default()));
                    }
                    if let Some(logo) = &self.logo { ui.image((logo.id(), logo.size_vec2() * (80.0 / logo.size_vec2().x))); }
                    ui.label(RichText::new("This product uses the TMDB API but is not endorsed or certified by TMDB.").small());
                });
                self.credits_open = credits.body_returned.is_some();
            });
        if response.should_close() || close {
            chosen = None;
            self.close();
        } else if chosen.is_some() {
            self.close();
        }
        chosen
    }
}

impl super::PlayerApp {
    pub(super) fn open_movie_search(&mut self) {
        self.show_url = false;
        self.show_help = false;
        self.movie_search.open();
    }
    fn play_searched_movie(&mut self, movie: Movie) {
        let source = movie.source();
        let index = self
            .queue
            .iter()
            .position(|m| m.source == source)
            .unwrap_or_else(|| {
                self.queue.push(super::Media {
                    source,
                    name: movie.label(),
                });
                self.queue.len() - 1
            });
        self.queue[index].name = movie.label();
        self.play(index);
    }
    pub(super) fn movie_search_dialog(&mut self, ctx: &egui::Context) {
        if let Some(movie) = self.movie_search.ui(ctx) {
            self.play_searched_movie(movie);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PlayerApp;
    use egui::{Event, Modifiers, RawInput};
    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    fn frame(
        app: &mut PlayerApp,
        ctx: &egui::Context,
        events: Vec<Event>,
        modifiers: Modifiers,
    ) -> egui::FullOutput {
        ctx.run(
            RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(440.0, 260.0),
                )),
                events,
                modifiers,
                ..Default::default()
            },
            |ctx| {
                app.shortcuts(ctx);
                app.movie_search_dialog(ctx);
            },
        )
    }
    fn movies() -> Vec<Movie> {
        vec![
            Movie {
                id: 210577,
                title: "Gone Girl".into(),
                year: "2014".into(),
                poster_path: None,
            },
            Movie {
                id: 1434964,
                title: "Gone Girl".into(),
                year: "2025".into(),
                poster_path: None,
            },
        ]
    }
    #[test]
    fn command_and_control_shift_m_focus_search_and_keyboard_selects_correct_movie() {
        for modifiers in [
            Modifiers {
                mac_cmd: true,
                command: true,
                shift: true,
                ..Default::default()
            },
            Modifiers {
                ctrl: true,
                command: true,
                shift: true,
                ..Default::default()
            },
        ] {
            let ctx = egui::Context::default();
            let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
            frame(&mut app, &ctx, vec![key(Key::M, modifiers)], modifiers);
            assert!(app.movie_search.open);
            assert!(ctx.memory(|m| m.has_focus(egui::Id::new("movie_search_query"))));
            frame(
                &mut app,
                &ctx,
                vec![Event::Text("Gone Girl".into())],
                Modifiers::NONE,
            );
            assert_eq!(app.movie_search.query, "Gone Girl");
            // A ready catalog response; keyboard selection still goes through
            // the actual dialog and app queue, without requiring the network.
            app.movie_search.edited = None;
            app.movie_search.results = movies();
            frame(
                &mut app,
                &ctx,
                vec![key(Key::ArrowDown, Modifiers::NONE)],
                Modifiers::NONE,
            );
            assert_eq!(app.movie_search.selected, 1);
            frame(
                &mut app,
                &ctx,
                vec![key(Key::Enter, Modifiers::NONE)],
                Modifiers::NONE,
            );
            assert!(!app.movie_search.open);
            assert_eq!(app.queue.len(), 1);
            assert_eq!(
                app.queue[0].source,
                "https://cinejoy.pk/watch/movie/1434964"
            );
            assert_eq!(app.queue[0].name, "Gone Girl (2025)");
        }
    }
    #[test]
    fn editing_discards_stale_results_and_escape_keeps_fullscreen() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.fullscreen = true;
        app.movie_search.open();
        app.movie_search.query = "Gone Girl".into();
        app.movie_search.results = movies();
        app.movie_search.query = "M".into();
        app.movie_search.changed();
        assert!(app.movie_search.results.is_empty());
        assert!(
            app.movie_search.edited.is_some(),
            "one-letter titles must be searchable"
        );
        frame(
            &mut app,
            &ctx,
            vec![key(Key::Escape, Modifiers::NONE)],
            Modifiers::NONE,
        );
        assert!(!app.movie_search.open);
        assert!(app.fullscreen);
    }
    #[test]
    fn search_results_fit_the_minimum_window_and_can_be_clicked() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.movie_search.open();
        app.movie_search.query = "Gone Girl".into();
        app.movie_search.results = movies();
        frame(&mut app, &ctx, vec![], Modifiers::NONE);
        let output = frame(&mut app, &ctx, vec![], Modifiers::NONE);
        let update = output.platform_output.accesskit_update.unwrap();
        for label in ["Search movies", "Movie data credits"] {
            let bounds = update
                .nodes
                .iter()
                .find_map(|(_, node)| {
                    (node.label() == Some(label) || node.value() == Some(label))
                        .then(|| node.bounds())
                        .flatten()
                })
                .expect(label);
            assert!(
                bounds.y0 >= 0.0 && bounds.y1 <= 260.0,
                "{label}: {bounds:?}"
            );
        }
        let bounds = update
            .nodes
            .iter()
            .find_map(|(_, node)| {
                (node.label() == Some("Gone Girl (2014)"))
                    .then(|| node.bounds())
                    .flatten()
            })
            .expect("movie result");
        assert!(
            bounds.x0 >= 0.0 && bounds.x1 <= 440.0 && bounds.y0 >= 0.0 && bounds.y1 <= 260.0,
            "{bounds:?}"
        );
        let pos = egui::pos2(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        );
        frame(
            &mut app,
            &ctx,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
            Modifiers::NONE,
        );
        frame(
            &mut app,
            &ctx,
            vec![Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
        );
        assert_eq!(app.queue[0].source, "https://cinejoy.pk/watch/movie/210577");
    }
    #[test]
    #[ignore = "requires libmpv, Deno, network movie search and Cinejoy"]
    fn real_search_dialog_starts_movie_and_preserves_title() {
        let ctx = egui::Context::default();
        let player = crate::playback::Player::for_ui_test(&ctx).unwrap();
        let mut app = PlayerApp::from_parts(&ctx, Some(player), None, None, vec![]);
        let modifiers = Modifiers::COMMAND | Modifiers::SHIFT;
        frame(&mut app, &ctx, vec![key(Key::M, modifiers)], modifiers);
        frame(
            &mut app,
            &ctx,
            vec![Event::Text("Gone Girl".into())],
            Modifiers::NONE,
        );
        frame(
            &mut app,
            &ctx,
            vec![key(Key::Enter, Modifiers::NONE)],
            Modifiers::NONE,
        );
        let deadline = Instant::now() + Duration::from_secs(75);
        while app.queue.is_empty() {
            frame(&mut app, &ctx, vec![], Modifiers::NONE);
            assert!(
                app.movie_search.error.is_none(),
                "{:?}",
                app.movie_search.error
            );
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!app.movie_search.open);
        assert_eq!(app.queue[0].source, "https://cinejoy.pk/watch/movie/210577");
        let deadline = Instant::now() + Duration::from_secs(60);
        while app.player.as_ref().unwrap().state.position < 3.0 {
            for event in app.player.as_mut().unwrap().poll() {
                if let crate::playback::PlaybackEvent::Error(error) = event {
                    panic!("{error}");
                }
            }
            assert!(Instant::now() < deadline, "searched movie did not play");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(app.player.as_ref().unwrap().state.title, "Gone Girl (2014)");
        app.stop();
    }
}
