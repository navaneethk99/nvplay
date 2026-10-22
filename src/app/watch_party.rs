use super::PlayerApp;
use crate::watch_together::{Event, Host, Snapshot};
use eframe::egui;
use watch_protocol::{Control, Playback};

pub(super) struct WatchParty {
    pub open: bool,
    host: Option<Host>,
    endpoint: String,
    key: String,
    url: String,
    viewers: usize,
    error: Option<String>,
    generation: u64,
    source: String,
}
impl Default for WatchParty {
    fn default() -> Self {
        let saved = crate::settings::directory()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join("watch-server.txt")).ok())
            .unwrap_or_default();
        Self {
            open: false,
            host: None,
            endpoint: std::env::var("NVPLAY_WATCH_URL").unwrap_or(saved),
            key: std::env::var("NVPLAY_WATCH_HOST_KEY").unwrap_or_else(|_| {
                crate::settings::directory()
                    .ok()
                    .and_then(|dir| std::fs::read_to_string(dir.join("watch-host-key.txt")).ok())
                    .map(|key| key.trim().to_owned())
                    .unwrap_or_default()
            }),
            url: String::new(),
            viewers: 0,
            error: None,
            generation: 0,
            source: String::new(),
        }
    }
}
impl WatchParty {
    pub(super) fn active(&self) -> bool {
        self.host.is_some()
    }
    pub(super) fn end(&mut self) {
        self.host = None;
        self.url.clear();
        self.viewers = 0;
        self.source.clear();
    }
}
impl PlayerApp {
    fn sharing_snapshot(&self) -> Option<Snapshot> {
        let player = self.player.as_ref()?;
        if player.state.idle || self.loading {
            return None;
        }
        let s = &player.state;
        let title = self
            .current
            .and_then(|i| self.queue.get(i))
            .map(|m| m.name.as_str())
            .filter(|t| !t.contains("://"))
            .unwrap_or(&s.title);
        let title = if title.contains("://") {
            "Shared video"
        } else {
            title
        };
        Some(Snapshot {
            source: self.watch_party.source.clone(),
            audio_source: None,
            video: s.tracks.iter().any(|t| t.kind == "video" && t.selected),
            audio: s.tracks.iter().any(|t| t.kind == "audio" && t.selected),
            state: Playback {
                position: s.position.max(0.0),
                duration: s.duration.max(0.0),
                paused: s.paused,
                buffering: s.buffering || s.seeking,
                speed: s.speed,
                title: title.chars().take(256).collect(),
            },
        })
    }
    fn start_watch_party(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let player = self.player.as_ref().ok_or("Open a video first.")?;
        if !player.state.seekable {
            return Err("Watch together currently needs a seekable video or audio file.".into());
        }
        if !(0.25..=4.0).contains(&player.state.speed) {
            return Err("Choose a playback speed between 0.25× and 4× before sharing.".into());
        }
        self.watch_party.source = player.sharing_source()?;
        let snapshot = self
            .sharing_snapshot()
            .ok_or("Wait for your video to load before sharing.")?;
        if !snapshot.state.valid() {
            return Err("Wait for playback information to finish loading.".into());
        }
        let ffmpeg = crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg").ok_or(
            "Install FFmpeg with libx264 and libopus to share video, then restart nvplay.",
        )?;
        self.watch_party.host = Some(Host::start(
            &self.watch_party.endpoint,
            self.watch_party.key.clone(),
            ffmpeg,
            snapshot,
            ctx.clone(),
        )?);
        self.watch_party.generation = self.playback_generation;
        self.watch_party.error = None;
        Ok(())
    }
    pub(super) fn poll_watch_party(&mut self) {
        if !self.watch_party.active() {
            return;
        }
        if self.playback_generation != self.watch_party.generation
            || self.player.as_ref().is_none_or(|p| p.state.idle)
        {
            self.watch_party.end();
            return;
        }
        let events = self
            .watch_party
            .host
            .as_ref()
            .map(Host::poll)
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Created(url) => self.watch_party.url = url,
                Event::Viewers(count) => self.watch_party.viewers = count,
                Event::Control(control) => match control {
                    Control::Play {} => self.command(&["set", "pause", "no"]),
                    Control::Pause {} => self.command(&["set", "pause", "yes"]),
                    Control::Seek { position } => {
                        if let Some(p) = self.player.as_ref().filter(|p| p.state.seekable) {
                            self.commit_seek(position.clamp(0.0, p.state.duration.max(0.0)));
                        }
                    }
                },
                Event::Error(error) => {
                    self.watch_party.error = Some(error);
                    self.watch_party.open = true;
                }
                Event::Closed => self.watch_party.end(),
            }
        }
        if let (Some(host), Some(snapshot)) = (&self.watch_party.host, self.sharing_snapshot()) {
            if snapshot.state.valid() {
                host.update(snapshot);
            } else {
                self.watch_party.error = Some(
                    "Sharing ended because the playback speed or timeline is unsupported.".into(),
                );
                self.watch_party.end();
            }
        }
    }
    pub(super) fn watch_party_dialog(&mut self, ctx: &egui::Context) {
        if !self.watch_party.open {
            return;
        }
        let mut open = true;
        egui::Window::new("Watch together")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(410.0)
            .show(ctx, |ui| {
                if self.watch_party.active() {
                    if self.watch_party.url.is_empty() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Creating your viewing link…");
                        });
                    } else {
                        ui.label("Send this link to your friends. They can join in a browser.");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.watch_party.url.as_str())
                                .desired_width(f32::INFINITY),
                        );
                        if ui.button("Copy viewing link").clicked() {
                            ctx.copy_text(self.watch_party.url.clone());
                        }
                        ui.label(format!(
                            "{} of {} viewers joined",
                            self.watch_party.viewers,
                            watch_protocol::MAX_VIEWERS
                        ));
                    }
                    ui.label("Everyone with the link can play, pause, and seek.");
                    ui.label("Keep nvplay open while sharing.");
                    if ui.button("End watch party").clicked() {
                        self.watch_party.end();
                    }
                } else {
                    ui.label("Watch your current video with up to six friends.");
                    ui.label("Everyone can play, pause, and seek together.");
                    if self.watch_party.endpoint.is_empty() {
                        ui.label("Configure your public watch service to create viewing links.");
                    }
                    ui.collapsing("Watch service settings", |ui| {
                        ui.label("Service address");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.watch_party.endpoint)
                                .hint_text("https://watch.example.com")
                                .desired_width(f32::INFINITY),
                        );
                        ui.label("Host key");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.watch_party.key)
                                .password(true)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    let available =
                        self.player.as_ref().is_some_and(|p| !p.state.idle) && !self.loading;
                    if ui
                        .add_enabled(available, egui::Button::new("Create viewing link"))
                        .clicked()
                    {
                        match self.start_watch_party(ctx) {
                            Err(error) => self.watch_party.error = Some(error),
                            Ok(()) => {
                                if let Ok(directory) = crate::settings::directory() {
                                    let _ = crate::settings::write_private(
                                        &directory.join("watch-server.txt"),
                                        self.watch_party.endpoint.as_bytes(),
                                    );
                                }
                            }
                        }
                    }
                    if !available {
                        ui.label("Open a video to start a watch party.");
                    }
                }
                if let Some(error) = &self.watch_party.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
            });
        self.watch_party.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires local watch server, FFmpeg, libmpv, and browser test driver"]
    fn real_watch_party_broadcasts_to_browsers() {
        use std::time::{Duration, Instant};
        let dir = std::path::PathBuf::from(
            std::env::var_os("NVPLAY_WATCH_TEST_DIR").expect("NVPLAY_WATCH_TEST_DIR"),
        );
        let media = std::env::var_os("NVPLAY_TEST_MEDIA").expect("NVPLAY_TEST_MEDIA");
        let ctx = egui::Context::default();
        let player = crate::playback::Player::for_ui_test(&ctx).unwrap();
        let mut app = PlayerApp::from_parts(&ctx, Some(player), None, None, vec![media]);
        app.watch_party.endpoint = std::env::var("NVPLAY_WATCH_TEST_URL")
            .unwrap_or_else(|_| "http://localhost:8787".into());
        app.watch_party.key = std::env::var("NVPLAY_WATCH_TEST_HOST_KEY").unwrap_or_default();
        let started = Instant::now();
        let mut began = false;
        while started.elapsed() < Duration::from_secs(150) {
            for event in app.player.as_mut().unwrap().poll() {
                app.handle_playback_event(event);
            }
            if !began
                && !app.loading
                && app
                    .player
                    .as_ref()
                    .is_some_and(|p| p.state.duration > 0.0 && !p.state.tracks.is_empty())
            {
                app.start_watch_party(&ctx).unwrap();
                began = true;
            }
            app.poll_watch_party();
            if let Some(error) = &app.watch_party.error {
                panic!("sharing failed: {error}");
            }
            if !app.watch_party.url.is_empty() && !dir.join("invite").exists() {
                std::fs::write(dir.join("invite"), &app.watch_party.url).unwrap();
            }
            if dir.join("end-session").exists() {
                app.watch_party.end();
                std::fs::write(dir.join("ended"), "ok").unwrap();
            }
            if dir.join("done").exists() {
                assert!(began);
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("browser driver did not complete the watch party test");
    }
}
