use crate::subtitles::library::{Library, Style};
use eframe::egui;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

pub(super) struct Preferences {
    pub library: Library,
    pending: BTreeMap<u64, Pending>,
    next_id: u64,
    save_after: Option<Instant>,
    pub error: Option<String>,
}
struct Pending {
    source: String,
    path: PathBuf,
    title: String,
    generation: u64,
}
impl Default for Preferences {
    fn default() -> Self {
        #[cfg(not(test))]
        let loaded = Library::load();
        #[cfg(test)]
        let loaded: Result<Library, String> = Ok(Library::memory());
        let (library, error) = match loaded {
            Ok(library) => (library, None),
            Err(error) => (Library::empty(), Some(error)),
        };
        Self {
            library,
            pending: BTreeMap::new(),
            next_id: 2,
            save_after: None,
            error,
        }
    }
}
impl Preferences {
    pub fn cancel_pending(&mut self) {
        self.pending.clear();
    }
    pub fn flush(&mut self) {
        if self.save_after.take().is_some() {
            self.error = self.library.save().err();
        }
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(deadline) = self.save_after {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.flush();
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }
}
impl super::PlayerApp {
    pub(super) fn open_subtitle_controls(&mut self) {
        self.show_playlist = true;
        self.sidebar_subtitles = true;
    }
    pub(super) fn add_subtitle(&mut self, path: PathBuf) {
        if self.loading || self.player.as_ref().is_none_or(|p| p.state.idle) {
            self.error =
                Some("Open a video and wait for it to load before adding subtitles.".into());
            return;
        }
        let Some(media) = self.current.and_then(|i| self.queue.get(i)) else {
            return;
        };
        let Some(filename) = path.to_str() else {
            self.error = Some("The subtitle path is not valid Unicode.".into());
            return;
        };
        let title = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let id = self.subtitle_preferences.next_id;
        self.subtitle_preferences.next_id = id.wrapping_add(1).max(2);
        match self
            .player
            .as_mut()
            .unwrap()
            .add_subtitle(filename, &title, id)
        {
            Ok(()) => {
                self.subtitle_preferences.pending.insert(
                    id,
                    Pending {
                        source: media.source.clone(),
                        path,
                        title,
                        generation: self.playback_generation,
                    },
                );
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub(super) fn subtitle_added(&mut self, id: u64, result: Result<(), String>) {
        let Some(pending) = self.subtitle_preferences.pending.remove(&id) else {
            return;
        };
        if pending.generation != self.playback_generation {
            return;
        }
        match result {
            Ok(()) => {
                self.command(&["set", "sub-visibility", "yes"]);
                self.subtitle_preferences.error = self
                    .subtitle_preferences
                    .library
                    .remember(&pending.source, &pending.path, &pending.title)
                    .err();
            }
            Err(error) => self.error = Some(format!("Could not add subtitle: {error}")),
        }
    }
    pub(super) fn apply_subtitle_style(&mut self) {
        for (name, value) in self.subtitle_preferences.library.style().properties() {
            self.command(&["set", name, &value]);
        }
    }
    pub(super) fn restore_subtitle(&mut self) {
        self.apply_subtitle_style();
        let saved = self
            .current
            .and_then(|i| self.queue.get(i))
            .and_then(|m| self.subtitle_preferences.library.remembered(&m.source));
        if let Some((path, title)) = saved {
            if !path.is_file() {
                self.subtitle_preferences.error = Some("The remembered subtitle is missing. Add it again to update this video's record.".into());
                return;
            }
            if let Some(path) = path.to_str() {
                self.command(&["sub-add", path, "select", &title]);
                self.command(&["set", "sub-visibility", "yes"]);
            }
        }
    }
    pub(super) fn subtitle_controls(&mut self, ui: &mut egui::Ui) {
        let mut style = self.subtitle_preferences.library.style().clone();
        let before = style.clone();
        egui::ScrollArea::vertical().id_salt("subtitle_controls").auto_shrink([false, false]).show(ui, |ui| {
            ui.heading("Subtitle appearance");
            ui.small("Changes apply immediately and are remembered.");
            ui.add_space(6.0);
            ui.label("Font family");
            ui.add(egui::TextEdit::singleline(&mut style.font).hint_text("Font name, e.g. Arial").char_limit(128).desired_width(f32::INFINITY));
            egui::ComboBox::from_id_salt("subtitle_font_presets").selected_text("Choose a font…").show_ui(ui, |ui| {
                for font in ["sans-serif", "serif", "monospace", "Arial", "Helvetica", "Verdana", "Georgia", "Times New Roman", "Courier New"] {
                    ui.selectable_value(&mut style.font, font.into(), font);
                }
            });
            ui.small("Use any installed font. Missing fonts use a fallback.");
            slider(ui, "Size", &mut style.size, 8.0..=120.0);
            ui.horizontal(|ui| { ui.checkbox(&mut style.bold, "Bold"); ui.checkbox(&mut style.italic, "Italic"); });
            color(ui, "Text color", &mut style.color);
            ui.separator();
            ui.strong("Stroke / outline");
            slider(ui, "Stroke width", &mut style.outline, 0.0..=10.0);
            color(ui, "Stroke color", &mut style.outline_color);
            ui.separator();
            ui.strong("Shadow and background");
            ui.checkbox(&mut style.background_box, "Background box");
            slider(ui, if style.background_box { "Box padding" } else { "Shadow size" }, &mut style.shadow, 0.0..=10.0);
            color(ui, if style.background_box { "Background color" } else { "Shadow color" }, &mut style.shadow_color);
            ui.separator();
            ui.strong("Placement");
            slider(ui, "Vertical position", &mut style.position, 0.0..=100.0);
            ui.small("0 = top · 100 = bottom");
            ui.horizontal(|ui| { ui.label("Alignment");
                egui::ComboBox::from_id_salt("subtitle_alignment").selected_text(&style.alignment).show_ui(ui, |ui| {
                    for value in ["left", "center", "right"] { ui.selectable_value(&mut style.alignment, value.into(), value); }
                });
            });
            ui.label("Vertical margin");
            ui.add(egui::Slider::new(&mut style.margin, 0..=150));
            slider(ui, "Letter spacing", &mut style.spacing, -10.0..=10.0);
            ui.separator();
            ui.checkbox(&mut style.override_styles, "Override subtitle file styles");
            ui.small("Text subtitles can be restyled. Bitmap subtitles keep their original font and colors.");
            if ui.button("Reset appearance").clicked() { style = Style::default(); }
            ui.separator();
            ui.strong("This video");
            let source = self.current.and_then(|i| self.queue.get(i)).map(|m| m.source.clone());
            if let Some(source) = source {
                if let Some((_, title)) = self.subtitle_preferences.library.remembered(&source) {
                    ui.label(format!("Remembered subtitle: {title}"));
                    if ui.button("Forget subtitle for this video").clicked() {
                        self.subtitle_preferences.error = self.subtitle_preferences.library.forget(&source).err();
                    }
                } else { ui.small("Added subtitles will be remembered for this video."); }
            } else { ui.small("Open a video to add and remember its subtitle."); }
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.dialog_busy, egui::Button::new("Add subtitle file…")).clicked() { self.pick(true); }
                if ui.button("Download subtitle…").clicked() { self.open_subtitle_search(); }
            });
            if let Some(error) = &self.subtitle_preferences.error { ui.colored_label(super::ACCENT, error); }
        });
        if style != before {
            self.subtitle_preferences.library.set_style(style);
            self.apply_subtitle_style();
            self.subtitle_preferences.save_after =
                Some(Instant::now() + Duration::from_millis(400));
            ui.ctx().request_repaint_after(Duration::from_millis(400));
        }
    }
}
fn slider(ui: &mut egui::Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.label(label);
    ui.add(egui::Slider::new(value, range));
}
fn color(ui: &mut egui::Ui, label: &str, rgba: &mut [u8; 4]) {
    ui.horizontal(|ui| {
        ui.color_edit_button_srgba_unmultiplied(rgba);
        ui.label(label);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{Media, PlayerApp},
        playback::Player,
        subtitles::library::tests::Fixture,
    };

    fn frame(
        app: &mut PlayerApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 500.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| app.ui(ctx),
        )
    }
    fn button(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        let bounds = output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find_map(|(_, node)| {
                (node.label() == Some(label))
                    .then(|| node.bounds())
                    .flatten()
            })
            .unwrap_or_else(|| panic!("Missing control: {label}"));
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
    fn pump(app: &mut PlayerApp) {
        for event in app.player.as_mut().unwrap().poll() {
            app.handle_playback_event(event);
        }
        assert!(app.error.is_none(), "{:?}", app.error);
    }
    fn wait(app: &mut PlayerApp, condition: impl Fn(&PlayerApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            pump(app);
            if condition(app) {
                break;
            }
            assert!(Instant::now() < deadline, "Playback condition timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn sidebar_customization_is_editable_and_saved() {
        let fixture = Fixture::new();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.subtitle_preferences.library = fixture.library();
        app.initial_resize = false;
        app.open_subtitle_controls();
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        assert!(app.show_playlist && app.sidebar_subtitles);
        let bold = button(&output, "Bold");
        assert!(bold.x < 300.0 && bold.y < 500.0);
        click(&mut app, &ctx, bold);
        assert!(app.subtitle_preferences.library.style().bold);
        assert!(app.subtitle_preferences.save_after.is_some());
        app.subtitle_preferences.flush();
        assert!(fixture.library().style().bold);
        // A family containing spaces must remain editable one keystroke at a time.
        let mut style = app.subtitle_preferences.library.style().clone();
        style.font = "Times ".into();
        app.subtitle_preferences.library.set_style(style);
        assert_eq!(app.subtitle_preferences.library.style().font, "Times ");
    }
    #[test]
    fn late_or_failed_additions_do_not_create_a_record() {
        let fixture = Fixture::new();
        let subtitle = fixture.0.join("subtitle.srt");
        std::fs::write(&subtitle, b"subtitle").unwrap();
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.subtitle_preferences.library = fixture.library();
        for (id, generation, result) in [(2, 0, Ok(())), (3, 1, Err("Invalid subtitle".into()))] {
            app.playback_generation = 1;
            app.subtitle_preferences.pending.insert(
                id,
                Pending {
                    source: "video".into(),
                    path: subtitle.clone(),
                    title: "Subtitle".into(),
                    generation,
                },
            );
            app.subtitle_added(id, result);
            assert!(
                app.subtitle_preferences
                    .library
                    .remembered("video")
                    .is_none()
            );
        }
        assert!(app.error.as_deref().unwrap().contains("Invalid subtitle"));
    }
    #[test]
    #[ignore = "requires libmpv and NVPLAY_TEST_MEDIA; uses isolated app settings, no OpenSubtitles connection"]
    fn real_subtitle_is_restored_after_restart_with_saved_appearance() {
        let fixture = Fixture::new();
        let source = std::env::var("NVPLAY_TEST_MEDIA").expect("NVPLAY_TEST_MEDIA");
        let subtitle = fixture.0.join("downloaded.srt");
        std::fs::write(
            &subtitle,
            b"1\n00:00:00,000 --> 00:01:00,000\nRemember this subtitle\n",
        )
        .unwrap();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(
            &ctx,
            Some(Player::for_ui_test(&ctx).unwrap()),
            None,
            None,
            vec![],
        );
        app.subtitle_preferences.library = fixture.library();
        app.queue.push(Media::new(source.clone()));
        app.play(0);
        wait(&mut app, |app| {
            !app.loading && !app.player.as_ref().unwrap().state.idle
        });
        // A failed engine addition must leave no remembered record.
        app.add_subtitle(fixture.0.join("missing.srt"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.subtitle_preferences.pending.is_empty() {
            for event in app.player.as_mut().unwrap().poll() {
                app.handle_playback_event(event);
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.error.is_some());
        assert!(
            app.subtitle_preferences
                .library
                .remembered(&source)
                .is_none()
        );
        app.error = None;
        app.add_subtitle(subtitle.clone());
        wait(&mut app, |app| {
            app.subtitle_preferences
                .library
                .remembered(&source)
                .is_some()
        });
        assert!(
            app.subtitle_preferences.error.is_none(),
            "{:?}",
            app.subtitle_preferences.error
        );
        app.open_subtitle_controls();
        app.initial_resize = false;
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, button(&output, "Bold"));
        let mut style = app.subtitle_preferences.library.style().clone();
        assert!(style.bold);
        style.font = "Arial".into();
        style.size = 56.0;
        style.color = [255, 32, 16, 128];
        style.outline = 4.5;
        style.outline_color = [12, 34, 56, 255];
        style.shadow = 2.0;
        style.shadow_color = [0, 0, 0, 128];
        style.position = 85.0;
        app.subtitle_preferences.library.set_style(style.clone());
        app.apply_subtitle_style();
        app.subtitle_preferences.flush();
        app.stop();
        drop(app);
        // Mimic the temporary download's cleanup on exit.
        std::fs::remove_file(&subtitle).unwrap();
        let ctx = egui::Context::default();
        let mut reopened = PlayerApp::from_parts(
            &ctx,
            Some(Player::for_ui_test(&ctx).unwrap()),
            None,
            None,
            vec![],
        );
        reopened.subtitle_preferences.library = fixture.library();
        reopened.queue.push(Media::new(source));
        reopened.play(0);
        wait(&mut reopened, |app| {
            !app.loading
                && app
                    .player
                    .as_ref()
                    .unwrap()
                    .state
                    .tracks
                    .iter()
                    .any(|track| track.kind == "sub" && track.selected)
                && app.player.as_ref().unwrap().state.subtitles_visible
        });
        assert_eq!(reopened.subtitle_preferences.library.style(), &style);
        let player = reopened.player.as_ref().unwrap();
        assert_eq!(player.test_property("sub-font").as_deref(), Some("Arial"));
        assert_eq!(
            player
                .test_property("sub-font-size")
                .unwrap()
                .parse::<f64>()
                .unwrap(),
            56.0
        );
        assert_eq!(
            player
                .test_property("sub-border-size")
                .unwrap()
                .parse::<f64>()
                .unwrap(),
            4.5
        );
        assert_eq!(
            player
                .test_property("sub-color")
                .unwrap()
                .to_ascii_uppercase(),
            "#80FF2010"
        );
        assert_eq!(
            player
                .test_property("sub-border-color")
                .unwrap()
                .to_ascii_uppercase(),
            "#FF0C2238"
        );
        assert_eq!(player.test_property("sub-bold").as_deref(), Some("yes"));
        // All style commands must have been accepted, including background mode.
        reopened.command(&["set", "sub-border-style", "background-box"]);
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            pump(&mut reopened);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            reopened
                .player
                .as_ref()
                .unwrap()
                .test_property("sub-border-style")
                .as_deref(),
            Some("background-box")
        );
        reopened.stop();
    }
}
