use super::PlayerApp;
use crate::watch_together::{Event, Host, Snapshot};
use eframe::egui;
use watch_protocol::{Control, Playback};

pub(super) struct WatchParty {
    pub(super) access: watch_protocol::Access,
    host: Option<Host>,
    endpoint: String,
    key: String,
    url: String,
    viewers: usize,
    error: Option<String>,
    status: String,
    generation: u64,
    source: String,
    broadcast: bool,
}
impl Default for WatchParty {
    fn default() -> Self {
        let saved = crate::settings::directory()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join("watch-server.txt")).ok())
            .unwrap_or_default();
        Self {
            access: Default::default(),
            host: None,
            endpoint: std::env::var("NVPLAY_WATCH_URL")
                .unwrap_or(saved)
                .trim()
                .to_owned(),
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
            status: "Creating your viewing link…".into(),
            generation: 0,
            source: String::new(),
            broadcast: false,
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
            video_headers: String::new(),
            audio_headers: String::new(),
            width: s.video_width,
            height: s.video_height,
            bitrate: 0,
            access: self.watch_party.access,
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
        let media = if self.watch_party.broadcast {
            None
        } else {
            player.sharing_browser_media()?
        };
        let ffmpeg = if media.is_none() {
            let tool = crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg")
                .ok_or("Install FFmpeg with libx264/libopus, then restart nvplay.")?;
            Some(tool)
        } else {
            None
        };
        self.watch_party.host = Some(Host::start(
            &self.watch_party.endpoint,
            self.watch_party.key.clone(),
            ffmpeg,
            media,
            snapshot,
            ctx.clone(),
        )?);
        self.watch_party.generation = self.playback_generation;
        self.watch_party.error = None;
        self.watch_party.status = "Creating your viewing link…".into();
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
                Event::Status(status) => self.watch_party.status = status,
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
                    self.show_playlist = true;
                    self.sidebar_tab = super::SidebarTab::WatchTogether;
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
    pub(super) fn watch_party_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.watch_party.active() {
            ui.strong("Session active");
            if ui
                .add_sized(
                    [ui.available_width(), 32.0],
                    egui::Button::new("End session")
                        .fill(egui::Color32::from_rgb(145, 40, 40)),
                )
                .on_hover_text("Stop sharing, disconnect all viewers, and expire the viewing link. Your video keeps playing.")
                .clicked()
            {
                self.watch_party.end();
            }
            ui.separator();
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("Supported internet videos play directly on each viewer’s device.");
            if let Some(p) = &self.player
                && p.state.video_width > 0 && p.state.video_height > 0 {
                ui.label(format!("{} × {} · Host video", p.state.video_width, p.state.video_height));
            }
            ui.small("Playback and seeking stay in sync. Local files are streamed from your device. Keep nvplay open while sharing.");
            ui.separator();
            ui.strong("Viewer access");
            ui.checkbox(&mut self.watch_party.access.play_pause, "Allow play and pause");
            ui.checkbox(&mut self.watch_party.access.seek, "Allow seeking");
            ui.checkbox(&mut self.watch_party.access.allow_join, "Allow new viewers to join");
            ui.small("Applies to everyone with the link. Changes take effect during the session. Volume and fullscreen stay personal.");
            ui.separator();
                if self.watch_party.active() {
                    if self.watch_party.url.is_empty() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(&self.watch_party.status);
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
                        ui.small(&self.watch_party.status);
                        ui.label(format!(
                            "{} of {} viewers joined",
                            self.watch_party.viewers,
                            watch_protocol::MAX_VIEWERS
                        ));
                    }

                } else {
                    ui.label("Watch your current video with up to six friends.");
                    ui.checkbox(&mut self.watch_party.broadcast, "Stream from my device instead")
                        .on_hover_text("Use this if a provider blocks direct browser playback. Requires more upload bandwidth and FFmpeg.");

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
                        match self.start_watch_party(&ctx) {
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
        app.watch_party.broadcast = true;
        let started = Instant::now();
        let mut began = false;
        let mut positioned = false;
        let mut state_second = u64::MAX;
        while started.elapsed() < Duration::from_secs(360) {
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
                if !positioned
                    && let Ok(position) = std::env::var("NVPLAY_WATCH_TEST_START_AT")
                        .unwrap_or_default()
                        .parse::<f64>()
                {
                    app.commit_seek(position);
                    positioned = true;
                    std::thread::sleep(Duration::from_millis(30));
                    continue;
                }
                if app.player.as_ref().unwrap().state.seeking {
                    continue;
                }
                if std::env::var("NVPLAY_WATCH_TEST_START_PAUSED").as_deref() == Ok("1")
                    && !app.player.as_ref().unwrap().state.paused
                {
                    app.command(&["set", "pause", "yes"]);
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                app.start_watch_party(&ctx).unwrap();
                began = true;
            }
            if dir.join("restrict-access").exists() {
                app.watch_party.access = watch_protocol::Access {
                    play_pause: false,
                    seek: false,
                    allow_join: false,
                };
            }
            if dir.join("restore-access").exists() {
                app.watch_party.access = Default::default();
            }
            app.poll_watch_party();
            if state_second != started.elapsed().as_secs() {
                state_second = started.elapsed().as_secs();
                if let Some(player) = &app.player {
                    let s = &player.state;
                    let state = serde_json::json!({"width": s.video_width, "height": s.video_height, "position": s.position, "paused": s.paused, "buffering": s.buffering, "seeking": s.seeking, "loading": app.loading, "video": s.tracks.iter().any(|t| t.kind == "video" && t.selected), "audio": s.tracks.iter().any(|t| t.kind == "audio" && t.selected)});
                    let _ = std::fs::write(dir.join("host-state.json"), state.to_string());
                }
            }
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
