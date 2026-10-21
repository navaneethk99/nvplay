mod icons;
mod movie_search;
#[cfg(target_os = "macos")]
mod native_menu;
mod posters;
mod preview;
#[cfg(target_os = "macos")]
mod updates;

use crate::{
    playback::{PlaybackEvent, Player, VideoRenderer},
    streaming,
};
use eframe::egui::{self, Color32, Key, RichText};
use icons::Icon;
use std::{
    ffi::OsString,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::{Duration, Instant},
};

const ACCENT: Color32 = Color32::from_rgb(217, 132, 34);
const BAR_BACKGROUND: Color32 = Color32::from_rgb(29, 29, 29);

#[derive(Clone)]
struct Media {
    source: String,
    name: String,
}
impl Media {
    fn new(source: String) -> Self {
        let name = if source.contains("://") {
            source.clone()
        } else {
            PathBuf::from(&source)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| source.clone())
        };
        Self { source, name }
    }
}
enum DialogResult {
    Media(Vec<PathBuf>),
    Subtitle(PathBuf),
    Folder(Result<Vec<PathBuf>, String>),
}

struct DialogWake(egui::Context);
impl Wake for DialogWake {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.request_repaint();
    }
}

pub struct PlayerApp {
    player: Option<Player>,
    renderer: Arc<Mutex<Option<VideoRenderer>>>,
    brand_logo: egui::TextureHandle,
    queue: Vec<Media>,
    current: Option<usize>,
    error: Option<String>,
    startup_error: bool,
    show_url: bool,
    movie_search: movie_search::MovieSearch,
    url: String,
    fullscreen: bool,
    repeat: bool,
    show_help: bool,
    show_playlist: bool,
    url_error: Option<String>,
    tools: streaming::Tools,
    last_title: String,
    initial_resize: bool,
    dialog: Option<Pin<Box<dyn Future<Output = DialogResult>>>>,
    dialog_busy: bool,
    seek_preview: Option<f64>,
    frame_preview: preview::Preview,
    loading: bool,
    ctx: egui::Context,
    open_documents: Option<crate::open_documents::OpenDocuments>,
    volume_preview: Option<f64>,
    pending_seek: Option<(f64, Instant)>,
    pending_volume: Option<(f64, Instant)>,
    chrome_until: Instant,
    fullscreen_request: Option<Instant>,
    show_info: bool,
    #[cfg(target_os = "macos")]
    native_menu: Option<native_menu::NativeMenu>,
    #[cfg(target_os = "macos")]
    updates: updates::Updates,
    #[cfg(test)]
    control_rects: ControlRects,
}

#[cfg(test)]
#[derive(Default)]
struct ControlRects {
    seek: Option<egui::Rect>,
    volume: Option<egui::Rect>,
    play: Option<egui::Rect>,
    skip_backward: Option<egui::Rect>,
    skip_forward: Option<egui::Rect>,
    mute: Option<egui::Rect>,
    stop: Option<egui::Rect>,
    preview_time: Option<egui::Rect>,
}

impl PlayerApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        sources: Vec<OsString>,
        open_documents: crate::open_documents::OpenDocuments,
    ) -> Self {
        theme(&cc.egui_ctx);
        open_documents.attach(&cc.egui_ctx);
        let result = Player::new(&cc.egui_ctx)
            .and_then(|player| VideoRenderer::new(&player, cc).map(|renderer| (player, renderer)));
        let (player, renderer, error) = match result {
            Ok((player, renderer)) => (Some(player), Some(renderer), None),
            Err(error) => (None, None, Some(error)),
        };
        let mut app = Self::from_parts(&cc.egui_ctx, player, renderer, error, sources);
        app.open_documents = Some(open_documents);
        #[cfg(target_os = "macos")]
        {
            app.native_menu = Some(native_menu::NativeMenu::new(&cc.egui_ctx));
            app.update_native_menu();
        }
        app
    }
    fn from_parts(
        ctx: &egui::Context,
        player: Option<Player>,
        renderer: Option<VideoRenderer>,
        error: Option<String>,
        sources: Vec<OsString>,
    ) -> Self {
        let startup_error = error.is_some();
        let mut app = Self {
            player,
            renderer: Arc::new(Mutex::new(renderer)),
            brand_logo: icons::load_logo(ctx),
            queue: Vec::new(),
            current: None,
            error,
            startup_error,
            show_url: false,
            movie_search: movie_search::MovieSearch::default(),
            url: String::new(),
            fullscreen: false,
            repeat: false,
            show_help: false,
            show_playlist: false,
            url_error: None,
            tools: streaming::Tools::discover(),
            last_title: String::new(),
            initial_resize: true,
            dialog: None,
            dialog_busy: false,
            seek_preview: None,
            frame_preview: preview::Preview::default(),
            loading: false,
            ctx: ctx.clone(),
            open_documents: None,
            volume_preview: None,
            pending_seek: None,
            pending_volume: None,
            chrome_until: Instant::now(),
            fullscreen_request: None,
            show_info: false,
            #[cfg(target_os = "macos")]
            native_menu: None,
            #[cfg(target_os = "macos")]
            updates: updates::Updates::default(),
            #[cfg(test)]
            control_rects: ControlRects::default(),
        };
        for source in sources {
            match source.into_string() {
                Ok(source) => app.queue.push(Media::new(source)),
                Err(_) => {
                    app.error = Some(
                        "This media path is not valid Unicode. Rename it to open it with nvplay."
                            .into(),
                    )
                }
            }
        }
        if !app.queue.is_empty() {
            app.play(0);
        }
        app
    }

    fn command(&mut self, args: &[&str]) {
        if let Some(player) = &mut self.player
            && let Err(error) = player.command(args)
        {
            self.error = Some(error);
        }
    }
    fn play(&mut self, index: usize) {
        if let (Some(media), Some(player)) = (self.queue.get(index), self.player.as_mut()) {
            match player.load(&media.source) {
                Ok(()) => {
                    if streaming::cinejoy::movie_id(&media.source).is_some()
                        && media.name != media.source
                    {
                        let _ = player.command(&["set", "force-media-title", &media.name]);
                    }
                    self.current = Some(index);
                    self.error = None;
                    self.seek_preview = None;
                    self.loading = true;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    fn next(&mut self) {
        if let Some(next) = next_index(self.current, self.queue.len(), self.repeat) {
            self.play(next);
        }
    }
    fn previous(&mut self) {
        if let Some(index) = self.current {
            self.play(index.saturating_sub(1));
        }
    }
    fn toggle_pause(&mut self) {
        if self.player.as_ref().is_some_and(|p| p.state.idle) {
            if let Some(index) = self
                .current
                .or(if self.queue.is_empty() { None } else { Some(0) })
            {
                self.play(index);
            }
        } else {
            self.command(&["cycle", "pause"]);
        }
    }
    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        self.fullscreen_request = Some(Instant::now());
        self.chrome_until = Instant::now() + Duration::from_secs(3);
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }
    fn pick(&mut self, subtitles: bool) {
        if self.dialog_busy {
            return;
        }
        self.dialog_busy = true;
        self.dialog = Some(if subtitles {
            Box::pin(async {
                rfd::AsyncFileDialog::new()
                    .set_title("Add subtitles")
                    .add_filter("Subtitles", &["srt", "ass", "ssa", "vtt", "sub"])
                    .pick_file()
                    .await
                    .map(|file| DialogResult::Subtitle(file.path().to_path_buf()))
                    .unwrap_or(DialogResult::Media(Vec::new()))
            })
        } else {
            Box::pin(async {
                let files = rfd::AsyncFileDialog::new()
                    .set_title("Open media — any format")
                    .pick_files()
                    .await
                    .unwrap_or_default();
                DialogResult::Media(files.iter().map(|file| file.path().to_path_buf()).collect())
            })
        });
        self.ctx.request_repaint();
    }
    fn pick_folder(&mut self) {
        if self.dialog_busy {
            return;
        }
        self.dialog_busy = true;
        let ctx = self.ctx.clone();
        self.dialog = Some(Box::pin(async move {
            let Some(folder) = rfd::AsyncFileDialog::new()
                .set_title("Add video folder to playlist")
                .pick_folder()
                .await
            else {
                return DialogResult::Folder(Ok(Vec::new()));
            };
            let path = folder.path().to_path_buf();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(folder_videos(&path));
                ctx.request_repaint();
            });
            let result = std::future::poll_fn(move |_| match receiver.try_recv() {
                Ok(result) => Poll::Ready(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => Poll::Pending,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Poll::Ready(Err("Could not finish reading the video folder.".into()))
                }
            })
            .await;
            DialogResult::Folder(result)
        }));
        self.ctx.request_repaint();
    }

    fn poll_dialog(&mut self) {
        let waker = Waker::from(Arc::new(DialogWake(self.ctx.clone())));
        let result = self
            .dialog
            .as_mut()
            .map(|future| future.as_mut().poll(&mut TaskContext::from_waker(&waker)));
        if let Some(Poll::Ready(result)) = result {
            self.dialog = None;
            self.dialog_busy = false;
            match result {
                DialogResult::Media(paths) => self.add_paths(paths),
                DialogResult::Folder(Ok(paths)) => self.enqueue_paths(paths, false),
                DialogResult::Folder(Err(error)) => self.error = Some(error),
                DialogResult::Subtitle(path) => match path.to_str() {
                    Some(path) => self.command(&["sub-add", path, "select"]),
                    None => self.error = Some("The subtitle path is not valid Unicode".into()),
                },
            }
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        self.enqueue_paths(paths, true);
    }

    fn enqueue_paths(&mut self, paths: Vec<PathBuf>, autoplay: bool) {
        let start = self.queue.len();
        for path in paths {
            match path.into_os_string().into_string() {
                Ok(source) => self.queue.push(Media::new(source)),
                Err(_) => {
                    self.error = Some(
                        "A file path is not valid Unicode. Rename that file to open it.".into(),
                    )
                }
            }
        }
        if autoplay && self.queue.len() > start {
            self.play(start);
        }
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx
            .input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::M))
        {
            self.open_movie_search();
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.movie_search.open {
                self.movie_search.close();
                return;
            }
            if self.show_url {
                self.show_url = false;
                return;
            }
            if self.show_help {
                self.show_help = false;
                return;
            }
            if self.show_info {
                self.show_info = false;
                return;
            }
            if self.fullscreen && !egui::Popup::is_any_open(ctx) {
                self.toggle_fullscreen(ctx);
                return;
            }
        }
        if ctx.wants_keyboard_input() || self.show_url || self.show_help || self.movie_search.open {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::O)) {
            self.pick(false);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::N)) {
            self.show_url = true;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::L)) {
            self.show_playlist = !self.show_playlist;
        }
        for (key, action) in [
            (Key::Space, 0),
            (Key::ArrowLeft, 1),
            (Key::ArrowRight, 2),
            (Key::F, 3),
            (Key::M, 4),
            (Key::ArrowUp, 5),
            (Key::ArrowDown, 6),
        ] {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                match action {
                    0 => self.toggle_pause(),
                    1 => self.command(&["seek", "-5", "relative+exact"]),
                    2 => self.command(&["seek", "5", "relative+exact"]),
                    3 => self.toggle_fullscreen(ctx),
                    4 => self.command(&["cycle", "mute"]),
                    5 | 6 => {
                        let volume =
                            self.pending_volume
                                .map(|pending| pending.0)
                                .unwrap_or_else(|| {
                                    self.player.as_ref().map_or(100.0, |p| p.state.volume)
                                });
                        self.set_volume(
                            (volume + if action == 5 { 5.0 } else { -5.0 }).clamp(0.0, 100.0),
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    fn stop(&mut self) {
        self.command(&["stop"]);
        self.loading = false;
        self.seek_preview = None;
        self.pending_seek = None;
    }

    #[cfg(not(target_os = "macos"))]
    fn menu(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu")
            .frame(
                egui::Frame::new()
                    .fill(BAR_BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(5, 1)),
            )
            .show(ctx, |ui| self.menu_contents(ui));
    }
    #[cfg(not(target_os = "macos"))]
    fn menu_contents(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Media", |ui| {
                if ui
                    .add_enabled(
                        !self.dialog_busy && self.player.is_some(),
                        egui::Button::new("Open File…").shortcut_text("Ctrl/Cmd+O"),
                    )
                    .clicked()
                {
                    self.pick(false);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.player.is_some(),
                        egui::Button::new("Open Network Stream…").shortcut_text("Ctrl/Cmd+N"),
                    )
                    .clicked()
                {
                    self.show_url = true;
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.player.is_some(),
                        egui::Button::new("Search Movies…").shortcut_text("Ctrl+Shift+M"),
                    )
                    .clicked()
                {
                    self.open_movie_search();
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Playback", |ui| {
                if ui.button("Play / Pause").clicked() {
                    self.toggle_pause();
                    ui.close();
                }
                if ui.button("Stop").clicked() {
                    self.stop();
                    ui.close();
                }
                if ui.button("Previous").clicked() {
                    self.previous();
                    ui.close();
                }
                if ui.button("Next").clicked() {
                    self.next();
                    ui.close();
                }
                ui.separator();
                ui.menu_button("Speed", |ui| {
                    let speed = self.player.as_ref().map_or(1.0, |p| p.state.speed);
                    for value in [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
                        if ui
                            .selectable_label((speed - value).abs() < 0.01, format!("{value:.2}×"))
                            .clicked()
                        {
                            self.command(&["set", "speed", &value.to_string()]);
                            ui.close();
                        }
                    }
                });
                ui.checkbox(&mut self.repeat, "Repeat playlist");
            });
            ui.menu_button("Audio", |ui| {
                ui.menu_button("Audio Track", |ui| self.track_menu(ui, "audio", "aid"));
                ui.menu_button("Audio Device", |ui| {
                    let (devices, current) = self
                        .player
                        .as_ref()
                        .map(|p| (p.state.audio_devices.clone(), p.state.audio_device.clone()))
                        .unwrap_or_default();
                    for device in devices {
                        if ui
                            .selectable_label(device.name == current, &device.description)
                            .clicked()
                        {
                            self.command(&["set", "audio-device", &device.name]);
                            ui.close();
                        }
                    }
                });
                let mut muted = self.player.as_ref().is_some_and(|p| p.state.muted);
                if ui.checkbox(&mut muted, "Mute").changed() {
                    self.command(&["set", "mute", if muted { "yes" } else { "no" }]);
                }
            });
            ui.menu_button("Video", |ui| {
                ui.menu_button("Video Track", |ui| self.track_menu(ui, "video", "vid"));
                if ui.button("Fullscreen").clicked() {
                    self.toggle_fullscreen(&ctx);
                    ui.close();
                }
            });
            ui.menu_button("Subtitle", |ui| {
                if ui.button("Add Subtitle File…").clicked() {
                    self.pick(true);
                    ui.close();
                }
                ui.menu_button("Subtitle Track", |ui| self.track_menu(ui, "sub", "sid"));
                let mut visible = self
                    .player
                    .as_ref()
                    .is_some_and(|p| p.state.subtitles_visible);
                if ui.checkbox(&mut visible, "Show subtitles").changed() {
                    self.command(&["set", "sub-visibility", if visible { "yes" } else { "no" }]);
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.show_playlist, "Playlist");
                ui.checkbox(&mut self.show_info, "Playback information");
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard shortcuts").clicked() {
                    self.show_help = true;
                    ui.close();
                }
            });
        });
    }

    fn playlist(&mut self, ctx: &egui::Context) {
        if !self.show_playlist {
            return;
        }
        let mut play = None;
        let mut remove = None;
        let mut move_item = None;
        egui::SidePanel::left("playlist_sidebar")
            .default_width(280.0)
            .width_range(200.0..=(ctx.available_rect().width() * 0.6).max(200.0))
            .resizable(true)
            .frame(egui::Frame::new().fill(BAR_BACKGROUND).inner_margin(8.0))
            .show(ctx, |ui| {
                if self.fullscreen {
                    // Fullscreen controls overlay the top and bottom of the video.
                    ui.add_space(if cfg!(target_os = "macos") {
                        34.0
                    } else {
                        58.0
                    });
                }
                ui.horizontal(|ui| {
                    ui.strong(format!("Playlist ({})", self.queue.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, Icon::Close, "Close playlist sidebar").clicked()
                        {
                            self.show_playlist = false;
                        }
                    });
                });
                ui.separator();
                if ui
                    .add_enabled(!self.dialog_busy, egui::Button::new("Add Folder…"))
                    .on_hover_text("Add videos from a folder and its subfolders")
                    .clicked()
                {
                    self.pick_folder();
                }
                ui.menu_button("Sort playlist", |ui| {
                    for (label, sort) in [
                        ("Name (A–Z)", PlaylistSort::Name),
                        ("Name (Z–A)", PlaylistSort::NameDescending),
                        ("Folder / path", PlaylistSort::Path),
                        ("File type", PlaylistSort::FileType),
                        ("Date modified (newest first)", PlaylistSort::Newest),
                        ("Date modified (oldest first)", PlaylistSort::Oldest),
                        ("Size (largest first)", PlaylistSort::Largest),
                        ("Size (smallest first)", PlaylistSort::Smallest),
                        ("Reverse order", PlaylistSort::Reverse),
                        ("Shuffle", PlaylistSort::Shuffle),
                    ] {
                        if ui.button(label).clicked() {
                            self.sort_playlist(sort);
                            ui.close();
                        }
                    }
                });
                if self.dialog_busy {
                    ui.label("Waiting for folder or scanning videos…");
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(
                        (ui.available_height() - if self.fullscreen { 34.0 } else { 0.0 }).max(0.0),
                    )
                    .show(ui, |ui| {
                        if self.queue.is_empty() {
                            ui.label(
                                "Playlist is empty. Add a folder, open a file or network stream.",
                            );
                        }
                        for (index, media) in self.queue.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let selected = self.current == Some(index);
                                let width = (ui.available_width() - 78.0).max(30.0);
                                if ui
                                    .add_sized(
                                        [width, 21.0],
                                        egui::Button::new(format!("{}  {}", index + 1, media.name))
                                            .selected(selected)
                                            .truncate(),
                                    )
                                    .on_hover_text(&media.source)
                                    .clicked()
                                {
                                    play = Some(index);
                                }
                                if ui
                                    .add_enabled_ui(index > 0, |ui| {
                                        icons::small_button(ui, Icon::Up, "Move up")
                                    })
                                    .inner
                                    .clicked()
                                {
                                    move_item = Some((index, index - 1));
                                }
                                if ui
                                    .add_enabled_ui(index + 1 < self.queue.len(), |ui| {
                                        icons::small_button(ui, Icon::Down, "Move down")
                                    })
                                    .inner
                                    .clicked()
                                {
                                    move_item = Some((index, index + 1));
                                }
                                if icons::small_button(ui, Icon::Close, "Remove").clicked() {
                                    remove = Some(index);
                                }
                            });
                        }
                    });
            });
        if let Some(index) = play {
            self.play(index);
        }
        if let Some((from, to)) = move_item {
            self.queue.swap(from, to);
            self.current = self.current.map(|current| {
                if current == from {
                    to
                } else if current == to {
                    from
                } else {
                    current
                }
            });
        }
        if let Some(index) = remove {
            self.queue.remove(index);
            match self.current {
                Some(current) if current == index => {
                    self.stop();
                    self.current = None;
                }
                Some(current) if current > index => self.current = Some(current - 1),
                _ => {}
            }
        }
    }

    fn sort_playlist(&mut self, sort: PlaylistSort) {
        let mut entries: Vec<_> = self
            .queue
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, media)| {
                let metadata = if matches!(
                    sort,
                    PlaylistSort::Newest
                        | PlaylistSort::Oldest
                        | PlaylistSort::Largest
                        | PlaylistSort::Smallest
                ) {
                    std::fs::metadata(&media.source).ok()
                } else {
                    None
                };
                let modified = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified().ok());
                let size = metadata.as_ref().map(|metadata| metadata.len());
                (index, media, modified, size)
            })
            .collect();
        match sort {
            PlaylistSort::Reverse => entries.reverse(),
            PlaylistSort::Shuffle => {
                let mut seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64
                    | 1;
                for index in (1..entries.len()).rev() {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    entries.swap(index, seed as usize % (index + 1));
                }
            }
            _ => entries.sort_by(|a, b| {
                let names = || natural_compare(&a.1.name, &b.1.name);
                match sort {
                    PlaylistSort::Name => names(),
                    PlaylistSort::NameDescending => names().reverse(),
                    PlaylistSort::Path => natural_compare(&a.1.source, &b.1.source),
                    PlaylistSort::FileType => {
                        let extension = |source: &str| {
                            PathBuf::from(source)
                                .extension()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_lowercase()
                        };
                        extension(&a.1.source)
                            .cmp(&extension(&b.1.source))
                            .then_with(names)
                    }
                    PlaylistSort::Newest => b.2.cmp(&a.2).then_with(names),
                    PlaylistSort::Oldest => {
                        a.2.is_none()
                            .cmp(&b.2.is_none())
                            .then(a.2.cmp(&b.2))
                            .then_with(names)
                    }
                    PlaylistSort::Largest => b.3.cmp(&a.3).then_with(names),
                    PlaylistSort::Smallest => {
                        a.3.is_none()
                            .cmp(&b.3.is_none())
                            .then(a.3.cmp(&b.3))
                            .then_with(names)
                    }
                    _ => std::cmp::Ordering::Equal,
                }
            }),
        }
        self.current = self
            .current
            .and_then(|current| entries.iter().position(|entry| entry.0 == current));
        self.queue = entries.into_iter().map(|entry| entry.1).collect();
    }

    fn topbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("topbar")
            .frame(
                egui::Frame::new()
                    .fill(BAR_BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(6, 3)),
            )
            .show(ctx, |ui| self.topbar_contents(ui));
    }

    fn topbar_contents(&mut self, ui: &mut egui::Ui) {
        bar_style(ui);
        let ctx = ui.ctx().clone();
        let idle = self.player.as_ref().is_none_or(|p| p.state.idle);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if icons::button(
                ui,
                Icon::Playlist,
                "Playlist sidebar (Ctrl/Cmd+L)",
                self.show_playlist,
            )
            .clicked()
            {
                self.show_playlist = !self.show_playlist;
            }
            let has_previous = self.current.is_some_and(|i| i > 0);
            let has_next = next_index(self.current, self.queue.len(), self.repeat).is_some();
            if ui
                .add_enabled_ui(has_previous, |ui| {
                    icons::button(ui, Icon::Previous, "Previous", false)
                })
                .inner
                .clicked()
            {
                self.previous();
            }
            if icons::button(ui, Icon::Repeat, "Repeat playlist", self.repeat).clicked() {
                self.repeat = !self.repeat;
            }
            if ui
                .add_enabled_ui(has_next, |ui| icons::button(ui, Icon::Next, "Next", false))
                .inner
                .clicked()
            {
                self.next();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::button(ui, Icon::Fullscreen, "Fullscreen (F)", self.fullscreen).clicked()
                {
                    self.toggle_fullscreen(&ctx);
                }
                let can_rotate = self.player.as_ref().is_some_and(|player| {
                    !player.state.idle
                        && player
                            .state
                            .tracks
                            .iter()
                            .any(|track| track.kind == "video" && track.selected)
                });
                if ui
                    .add_enabled_ui(can_rotate, |ui| {
                        icons::button(ui, Icon::RotateClockwise, "Rotate clockwise 90°", false)
                    })
                    .inner
                    .clicked()
                {
                    self.command(&["cycle-values", "video-rotate", "90", "180", "270", "0"]);
                }
                let stop = ui
                    .add_enabled_ui(self.loading || !idle, |ui| {
                        icons::button(ui, Icon::Stop, "Stop / Cancel stream", false)
                    })
                    .inner;
                #[cfg(test)]
                {
                    self.control_rects.stop = Some(stop.rect);
                }
                if stop.clicked() {
                    self.stop();
                }
            });
        });
    }

    fn controls(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("controls")
            .frame(
                egui::Frame::new()
                    .fill(BAR_BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(6, 3)),
            )
            .show(ctx, |ui| self.controls_contents(ui));
    }
    fn commit_seek(&mut self, seconds: f64) {
        self.command(&["seek", &seconds.to_string(), "absolute+exact"]);
        self.pending_seek = Some((seconds, Instant::now()));
        self.seek_preview = None;
    }
    fn set_volume(&mut self, volume: f64) {
        self.command(&["set", "volume", &volume.to_string()]);
        self.pending_volume = Some((volume, Instant::now()));
        self.ctx.request_repaint();
    }
    fn controls_contents(&mut self, ui: &mut egui::Ui) {
        bar_style(ui);
        let ctx = ui.ctx().clone();
        let state = self.player.as_ref().map(|p| &p.state);
        let (position, duration, paused, volume, idle, buffering, seekable, muted) = state
            .map(|s| {
                (
                    s.position,
                    s.duration,
                    s.paused,
                    s.volume,
                    s.idle,
                    s.buffering,
                    s.seekable,
                    s.muted,
                )
            })
            .unwrap_or((0.0, 0.0, true, 100.0, true, false, false, false));
        let display_position = self
            .seek_preview
            .or(self.pending_seek.map(|p| p.0))
            .unwrap_or(position);
        let display_volume = self
            .volume_preview
            .or(self.pending_volume.map(|p| p.0))
            .unwrap_or(volume);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let show_volume_value = ui.available_width() >= 560.0;
            let volume_width = if show_volume_value { 48.0 } else { 32.0 };
            let show_duration = ui.available_width() >= 500.0;
            let right_width = 31.0
                + volume_width
                + if show_volume_value { 35.0 } else { 0.0 }
                + if self.loading || buffering { 19.0 } else { 0.0 };
            let timeline_width = (ui.available_width() - 90.0 - right_width - 12.0).max(20.0);
            let can_skip = seekable && !idle && !self.loading;
            let backward = ui
                .add_enabled_ui(can_skip, |ui| {
                    icons::button(ui, Icon::SkipBackward, "Skip backward 5 seconds (←)", false)
                })
                .inner;
            #[cfg(test)]
            {
                self.control_rects.skip_backward = Some(backward.rect);
            }
            if backward.clicked() {
                self.command(&["seek", "-5", "relative+exact"]);
            }
            let play = ui
                .add_enabled_ui(self.player.is_some(), |ui| {
                    icons::button(
                        ui,
                        if paused || idle {
                            Icon::Play
                        } else {
                            Icon::Pause
                        },
                        "Play / Pause (Space)",
                        false,
                    )
                })
                .inner;
            #[cfg(test)]
            {
                self.control_rects.play = Some(play.rect);
            }
            if play.clicked() {
                if self.current.is_none() && self.queue.is_empty() {
                    self.pick(false);
                } else {
                    self.toggle_pause();
                }
            }
            let forward = ui
                .add_enabled_ui(can_skip, |ui| {
                    icons::button(ui, Icon::SkipForward, "Skip forward 5 seconds (→)", false)
                })
                .inner;
            #[cfg(test)]
            {
                self.control_rects.skip_forward = Some(forward.rect);
            }
            if forward.clicked() {
                self.command(&["seek", "5", "relative+exact"]);
            }
            ui.add_space(3.0);
            ui.allocate_ui_with_layout(
                egui::vec2(timeline_width, 28.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.visuals_mut().selection.bg_fill = ACCENT;
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let elapsed = format_time(display_position);
                    let remaining = if !idle && duration <= 0.0 {
                        "LIVE".into()
                    } else {
                        format_time(duration)
                    };
                    let time_width = |text: &str| {
                        ui.painter()
                            .layout_no_wrap(
                                text.into(),
                                egui::FontId::monospace(11.0),
                                Color32::GRAY,
                            )
                            .size()
                            .x
                    };
                    let seek_width = (timeline_width
                        - time_width(&elapsed)
                        - if show_duration {
                            time_width(&remaining) + 8.0
                        } else {
                            4.0
                        })
                    .max(20.0);
                    ui.label(RichText::new(elapsed).monospace().size(11.0));
                    let mut seek = display_position;
                    let can_seek = seekable && duration > 0.0 && !idle && !self.loading;
                    ui.spacing_mut().slider_width = seek_width;
                    ui.spacing_mut().interact_size.y = 28.0;
                    ui.spacing_mut().slider_rail_height = 28.0;
                    let response = ui.add_enabled(
                        can_seek,
                        egui::Slider::new(&mut seek, 0.0..=duration.max(1.0))
                            .show_value(false)
                            .trailing_fill(true)
                            .smart_aim(false)
                            // Quantize user edits only. Rounding an observed
                            // engine timestamp marks the slider as changed and
                            // used to issue an unintended seek on every frame.
                            .clamping(egui::SliderClamping::Edits)
                            .max_decimals(3)
                            .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.4 }),
                    );
                    #[cfg(test)]
                    {
                        self.control_rects.seek = Some(response.rect);
                    }
                    if response.changed() {
                        if response.dragged() {
                            self.seek_preview = Some(seek);
                        } else {
                            self.commit_seek(seek);
                        }
                    }
                    if response.drag_stopped() {
                        self.commit_seek(seek);
                    }
                    if can_seek && response.hovered() && !ui.input(|input| input.pointer.any_down())
                    {
                        let delta = ui.input_mut(|input| {
                            let delta = input.raw_scroll_delta;
                            input.raw_scroll_delta = egui::Vec2::ZERO;
                            input.smooth_scroll_delta = egui::Vec2::ZERO;
                            delta.x - delta.y
                        });
                        if delta != 0.0 {
                            self.commit_seek(
                                (display_position + f64::from(delta) / 20.0).clamp(0.0, duration),
                            );
                        }
                    }
                    if let Some(pointer) = response
                        .hover_pos()
                        .or_else(|| response.interact_pointer_pos())
                    {
                        if can_seek {
                            // Match egui's rectangular slider handle travel exactly.
                            let inset = response.rect.height() / 2.5 * 0.4;
                            let fraction = ((pointer.x - response.rect.left() - inset)
                                / (response.rect.width() - 2.0 * inset).max(1.0))
                            .clamp(0.0, 1.0);
                            let seconds =
                                (duration * f64::from(fraction)).min((duration - 0.05).max(0.0));
                            let source = self
                                .current
                                .and_then(|index| self.queue.get(index))
                                .map(|media| media.source.clone());
                            let video_track = self
                                .player
                                .as_ref()
                                .map(|p| {
                                    p.state
                                        .tracks
                                        .iter()
                                        .filter(|track| track.kind == "video")
                                        .position(|track| track.selected)
                                        .unwrap_or(0)
                                })
                                .unwrap_or(0);
                            self.timeline_preview(
                                &ctx,
                                response.id,
                                pointer.x,
                                response.rect.top(),
                                source.as_deref(),
                                seconds,
                                video_track,
                            );
                        } else {
                            response.on_hover_text(if idle {
                                "Open media to seek"
                            } else {
                                "This stream does not support seeking"
                            });
                        }
                    }
                    if show_duration {
                        ui.label(RichText::new(remaining).monospace().size(11.0));
                    }
                },
            );
            ui.add_space(3.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.visuals_mut().selection.bg_fill = ACCENT;
                ui.spacing_mut().slider_width = volume_width;
                let mut volume = display_volume;
                if show_volume_value {
                    ui.add_sized(
                        [35.0, 28.0],
                        egui::Label::new(
                            RichText::new(format!("{volume:.0}%"))
                                .monospace()
                                .size(10.0)
                                .color(if muted {
                                    Color32::GRAY
                                } else {
                                    Color32::from_gray(200)
                                }),
                        )
                        .halign(egui::Align::RIGHT),
                    );
                }
                let response = ui.add_enabled(
                    self.player.is_some(),
                    egui::Slider::new(&mut volume, 0.0..=100.0)
                        .show_value(false)
                        .trailing_fill(true),
                );
                #[cfg(test)]
                {
                    self.control_rects.volume = Some(response.rect);
                }
                if response.changed() {
                    self.volume_preview = Some(volume);
                    self.set_volume(volume);
                }
                if !response.dragged() {
                    self.volume_preview = None;
                }
                response.on_hover_text(format!(
                    "Volume: {display_volume:.0}%{}",
                    if muted { " (muted)" } else { "" }
                ));
                let mute = ui
                    .add_enabled_ui(self.player.is_some(), |ui| {
                        icons::button(
                            ui,
                            if muted { Icon::Muted } else { Icon::Volume },
                            "Mute (M)",
                            muted,
                        )
                    })
                    .inner;
                #[cfg(test)]
                {
                    self.control_rects.mute = Some(mute.rect);
                }
                if mute.clicked() {
                    self.command(&["cycle", "mute"]);
                }
                if self.loading || buffering {
                    ui.spinner();
                }
            });
        });
    }

    fn timeline_preview(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        pointer_x: f32,
        timeline_top: f32,
        source: Option<&str>,
        seconds: f64,
        track: usize,
    ) {
        let screen = ctx.content_rect();
        let card_size = egui::vec2(174.0, 126.0);
        let left = (pointer_x - card_size.x / 2.0).clamp(
            screen.left(),
            (screen.right() - card_size.x).max(screen.left()),
        );
        let top = (timeline_top - card_size.y - 6.0).max(screen.top());
        egui::Area::new(id.with("frame-preview"))
            .order(egui::Order::Tooltip)
            .fixed_pos(egui::pos2(left, top))
            .movable(false)
            .interactable(false)
            .constrain(false)
            .fade_in(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.set_width(160.0);
                        ui.set_min_height(112.0);
                        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(160.0, 90.0),
                                egui::Layout::top_down(egui::Align::Center),
                                |ui| {
                                    if let Some(source) = source {
                                        self.frame_preview.show(ui, source, seconds, track);
                                    }
                                },
                            );
                            let timestamp = ui.add_sized(
                                [160.0, 19.0],
                                egui::Label::new(
                                    RichText::new(format!(
                                        "{}.{:03}",
                                        format_time(seconds),
                                        (seconds.fract() * 1000.0).floor() as u32
                                    ))
                                    .monospace(),
                                ),
                            );
                            #[cfg(test)]
                            {
                                self.control_rects.preview_time = Some(timestamp.rect);
                            }
                            #[cfg(not(test))]
                            let _ = timestamp;
                        });
                    });
            });
    }

    #[cfg(not(target_os = "macos"))]
    fn track_menu(&mut self, ui: &mut egui::Ui, kind: &str, property: &str) {
        let tracks = self
            .player
            .as_ref()
            .map(|p| {
                p.state
                    .tracks
                    .iter()
                    .filter(|t| t.kind == kind)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if tracks.is_empty() {
            ui.label(format!("No {kind} tracks"));
            return;
        }
        if ui
            .selectable_label(!tracks.iter().any(|t| t.selected), "Disable")
            .clicked()
        {
            self.command(&["set", property, "no"]);
            ui.close();
        }
        for track in tracks {
            if ui.selectable_label(track.selected, track.label()).clicked() {
                self.command(&["set", property, &track.id.to_string()]);
                ui.close();
            }
        }
    }

    fn fullscreen_controls(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("fullscreen_topbar"))
            .order(egui::Order::Foreground)
            .movable(false)
            .fixed_pos(screen.left_top())
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(BAR_BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.set_width(screen.width() - 12.0);
                        #[cfg(not(target_os = "macos"))]
                        self.menu_contents(ui);
                        self.topbar_contents(ui);
                    });
            });
        egui::Area::new(egui::Id::new("fullscreen_transport"))
            .order(egui::Order::Foreground)
            .movable(false)
            .fixed_pos(egui::pos2(screen.left(), screen.bottom() - 34.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(BAR_BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.set_width(screen.width() - 12.0);
                        self.controls_contents(ui);
                    });
            });
    }

    fn video(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
                ui.painter()
                    .add(VideoRenderer::callback(self.renderer.clone(), rect));
                if self.current.is_none() || self.player.as_ref().is_none_or(|p| p.state.idle) {
                    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
                    icons::logo(ui.painter(), rect, &self.brand_logo);
                }
                if response.double_clicked() {
                    self.toggle_fullscreen(ctx);
                }
                response.context_menu(|ui| {
                    if ui.button("Play / Pause").clicked() {
                        self.toggle_pause();
                        ui.close();
                    }
                    if ui.button("Open File…").clicked() {
                        self.pick(false);
                        ui.close();
                    }
                    if ui.button("Open Network Stream…").clicked() {
                        self.show_url = true;
                        ui.close();
                    }
                    if ui.button("Search Movies…").clicked() {
                        self.open_movie_search();
                        ui.close();
                    }
                    if ui.button("Fullscreen").clicked() {
                        self.toggle_fullscreen(ctx);
                        ui.close();
                    }
                });
            });
    }

    fn network_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_url {
            return;
        }
        let mut open = true;
        egui::Window::new("Open Network Stream")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.label("Please enter a network URL:");
                let input = ui.add_sized(
                    [380.0, 24.0],
                    egui::TextEdit::singleline(&mut self.url)
                        .hint_text("YouTube link or a direct video / live stream URL"),
                );
                if input.changed() {
                    self.url_error = None;
                }
                if !input.has_focus() && self.url.is_empty() {
                    input.request_focus();
                }
                ui.add_space(3.0);
                ui.label(
                    RichText::new(
                        "YouTube and supported sites play here. Netflix / DRM is unsupported.",
                    )
                    .size(10.0)
                    .color(Color32::from_gray(90)),
                );
                if let Some(player) = &mut self.player {
                    egui::ComboBox::from_label("Stream quality")
                        .selected_text(player.stream_quality.label())
                        .show_ui(ui, |ui| {
                            for quality in streaming::Quality::ALL {
                                ui.selectable_value(&mut player.stream_quality, quality, quality.label());
                            }
                        });
                    ui.small("Choose Data saver for slow connections. Quality selects website formats and HLS variants; fixed-quality links are unchanged.");
                }
                if self.tools.ytdlp.is_none() {
                    ui.colored_label(
                        Color32::from_rgb(165, 73, 0),
                        "Website playback needs yt-dlp. Direct video URLs work without it.",
                    );
                }
                if let Some(error) = &self.url_error {
                    ui.colored_label(Color32::from_rgb(175, 30, 20), error);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.url.trim().is_empty() && self.player.is_some(),
                            egui::Button::new("Play"),
                        )
                        .clicked()
                        || (input.lost_focus()
                            && ui.input(|i| i.key_pressed(Key::Enter))
                            && self.player.is_some())
                    {
                        match streaming::network_url(&self.url) {
                            Ok(source) => {
                                self.queue.push(Media::new(source));
                                self.play(self.queue.len() - 1);
                                self.url.clear();
                                self.url_error = None;
                                self.show_url = false;
                            }
                            Err(error) => self.url_error = Some(error),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_url = false;
                    }
                });
            });
        self.show_url &= open;
    }
}

impl PlayerApp {
    fn information(&mut self, ctx: &egui::Context) {
        if !self.show_info {
            return;
        }
        egui::Window::new("Playback information")
            .open(&mut self.show_info)
            .default_width(310.0)
            .show(ctx, |ui| {
                if let Some(player) = &self.player {
                    let state = &player.state;
                    ui.label(if state.idle {
                        "Stopped"
                    } else if state.buffering {
                        "Buffering"
                    } else if state.paused {
                        "Paused"
                    } else {
                        "Playing"
                    });
                    ui.label(format!(
                        "{} / {}",
                        format_time(state.position),
                        format_time(state.duration)
                    ));
                    ui.label(format!(
                        "Seeking: {}",
                        if state.seekable {
                            "available"
                        } else {
                            "unavailable"
                        }
                    ));
                    ui.separator();
                    ui.label(format!(
                        "Audio: {} · {} channels",
                        if state.audio_codec.is_empty() {
                            "none"
                        } else {
                            &state.audio_codec
                        },
                        state.audio_channels
                    ));
                    ui.label(format!(
                        "Volume: {:.0}%{}",
                        state.volume,
                        if state.muted { " (muted)" } else { "" }
                    ));
                    ui.label(format!(
                        "Audio track: {} · Subtitles: {}",
                        state.audio_track, state.subtitle_track
                    ));
                    let device = state
                        .audio_devices
                        .iter()
                        .find(|d| d.name == state.audio_device)
                        .map_or(state.audio_device.as_str(), |d| d.description.as_str());
                    ui.label(format!("Output: {device}"));
                    ui.label(format!("Speed: {:.2}×", state.speed));
                    ui.separator();
                    ui.label(format!(
                        "Video: {}",
                        if state.video_codec.is_empty() {
                            "none"
                        } else {
                            &state.video_codec
                        }
                    ));
                    ui.label(format!(
                        "Resolution: {} × {}",
                        state.video_width, state.video_height
                    ));
                    ui.label(format!(
                        "Decoder: {}",
                        if state.hardware_decoder.is_empty() || state.hardware_decoder == "no" {
                            "software"
                        } else {
                            &state.hardware_decoder
                        }
                    ));
                    ui.label(format!(
                        "Dropped frames: {} · Decoder drops: {}",
                        state.dropped_frames, state.decoder_dropped_frames
                    ));
                    ui.label(format!(
                        "Buffered: {:.1} s · Read rate: {:.2} MB/s",
                        state.cache_seconds,
                        state.cache_bytes_per_second as f64 / 1_000_000.0
                    ));

                    if let Ok(renderer) = self.renderer.lock()
                        && let Some(renderer) = &*renderer
                    {
                        ui.separator();
                        ui.label(format!(
                            "Video frames: {} · Cached redraws: {}",
                            renderer.frames_rendered, renderer.frames_reused
                        ));
                        ui.label(format!(
                            "Longest video render: {:.1} ms",
                            renderer.max_render_ms
                        ));
                    }
                }
            });
    }

    fn ui(&mut self, ctx: &egui::Context) {
        // macOS may restore a previous native window frame when relaunching the
        // bundle. Apply the compact size after creation as well as in the builder.
        if self.initial_resize {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(640.0, 400.0)));
            self.initial_resize = false;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        if self.pending_seek.is_some() || self.pending_volume.is_some() {
            ctx.request_repaint_after(Duration::from_millis(25));
        }
        if let Some(actual) = ctx.input(|i| i.viewport().fullscreen)
            && (actual == self.fullscreen
                || self
                    .fullscreen_request
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(1)))
        {
            self.fullscreen = actual;
            self.fullscreen_request = None;
        }
        self.poll_dialog();
        let opened = self
            .open_documents
            .as_ref()
            .map(|events| events.take())
            .unwrap_or_default();
        if !opened.is_empty() {
            self.add_paths(opened);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        let events = self.player.as_mut().map(Player::poll).unwrap_or_default();
        for event in events {
            match event {
                PlaybackEvent::Finished => {
                    self.loading = false;
                    self.next();
                }
                PlaybackEvent::Error(error) => {
                    self.error = Some(error);
                    self.loading = false;
                }
                PlaybackEvent::Loaded => self.loading = false,
            }
        }
        let drops = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect::<Vec<_>>()
        });
        if !drops.is_empty() {
            self.add_paths(drops);
        }
        #[cfg(target_os = "macos")]
        self.update_native_menu();
        // Keep presentation on eframe's vsync cadence while video is playing.
        // Event-only redraws can miss a refresh when decode wakeups arrive
        // close to a display deadline. Paused video still reuses its texture.
        if self.player.as_ref().is_some_and(|player| {
            !player.state.idle
                && !player.state.paused
                && !player.state.buffering
                && player
                    .state
                    .tracks
                    .iter()
                    .any(|track| track.kind == "video" && track.selected)
        }) {
            ctx.request_repaint();
        }
        self.shortcuts(ctx);
        let title = self
            .player
            .as_ref()
            .map(|p| p.state.title.as_str())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                self.current
                    .and_then(|i| self.queue.get(i))
                    .map(|m| m.name.as_str())
            })
            .map_or_else(|| "nvplay".to_owned(), |name| format!("{name} — nvplay"));
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        if let Some(player) = &self.player {
            if self.pending_seek.is_some_and(|(target, time)| {
                (player.state.position - target).abs() < 0.3
                    || time.elapsed() > Duration::from_secs(2)
            }) {
                self.pending_seek = None;
            }
            if self.pending_volume.is_some_and(|(target, time)| {
                (player.state.volume - target).abs() < 0.1
                    || time.elapsed() > Duration::from_secs(1)
            }) {
                self.pending_volume = None;
            }
        }
        if let Some(player) = &self.player {
            let track = player
                .state
                .tracks
                .iter()
                .filter(|track| track.kind == "video")
                .position(|track| track.selected)
                .unwrap_or(0);
            if let Some(media) = self.current.and_then(|index| self.queue.get(index)) {
                self.frame_preview
                    .prepare(ctx, &media.source, player.state.duration, track);
            } else {
                self.frame_preview.prepare(ctx, "", 0.0, 0);
            }
        }
        let now = Instant::now();
        let (active, near_edge) = ctx.input(|i| {
            (
                i.pointer.delta() != egui::Vec2::ZERO
                    || i.pointer.any_down()
                    || i.key_pressed(Key::F),
                i.pointer
                    .hover_pos()
                    .is_some_and(|p| p.y < 30.0 || p.y > i.content_rect().height() - 46.0),
            )
        });
        let pinned = egui::Popup::is_any_open(ctx)
            || self.show_url
            || self.movie_search.open
            || self.show_help
            || self.show_info
            || self.show_playlist
            || self.seek_preview.is_some()
            || self.volume_preview.is_some();
        let chrome_visible = chrome_visible(
            self.fullscreen,
            now,
            &mut self.chrome_until,
            active,
            near_edge,
            pinned,
        );
        if !self.fullscreen {
            #[cfg(not(target_os = "macos"))]
            self.menu(ctx);
            self.topbar(ctx);
            self.controls(ctx);
        }
        self.playlist(ctx);
        self.video(ctx);
        if self.fullscreen && chrome_visible {
            self.fullscreen_controls(ctx);
        }
        if self.fullscreen && !pinned && !near_edge && now < self.chrome_until {
            ctx.request_repaint_after(self.chrome_until.saturating_duration_since(now));
        }
        if let Ok(mut renderer) = self.renderer.lock()
            && let Some(renderer) = renderer.as_mut()
            && let Some(error) = renderer.error.take()
        {
            self.error = Some(format!("Video rendering failed: {error}"));
        }
        self.information(ctx);

        #[cfg(target_os = "macos")]
        self.updates.ui(ctx);
        self.network_dialog(ctx);
        self.movie_search_dialog(ctx);
        if self.show_help {
            egui::Window::new("Keyboard shortcuts")
                .open(&mut self.show_help)
                .collapsible(false)
                .show(ctx, |ui| {
                    egui::Grid::new("shortcuts")
                        .spacing([30.0, 12.0])
                        .show(ui, |ui| {
                            for (key, action) in [
                                ("Ctrl / Cmd + O", "Open media"),
                                ("Ctrl / Cmd + N", "Open network stream"),
                                ("Ctrl / Cmd + Shift + M", "Search movies"),
                                ("Ctrl / Cmd + L", "Toggle playlist"),
                                ("Space", "Play / pause"),
                                ("← / →", "Seek 5 seconds"),
                                ("↑ / ↓", "Adjust volume"),
                                ("M", "Mute / unmute"),
                                ("F / double click video", "Toggle fullscreen"),
                                ("Escape", "Leave fullscreen"),
                            ] {
                                ui.label(RichText::new(key).color(ACCENT));
                                ui.label(action);
                                ui.end_row();
                            }
                        });
                });
        }
        if let Some(error) = self.error.clone() {
            let mut open = true;
            egui::Window::new(if self.startup_error { "Playback engine unavailable" } else { "Playback error" }).open(&mut open).default_width(520.0).show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| { ui.label(&error); });
                if self.startup_error { ui.separator(); ui.label("macOS: brew install mpv\nUbuntu / Debian: sudo apt install libmpv2\nWindows: put mpv-2.dll and its dependencies beside nvplay.exe.\n\nRestart nvplay after installing the library."); }
                if ui.button("Dismiss").clicked() { self.error = None; }
            });
            if !open {
                self.error = None;
            }
        }
    }
}
fn chrome_visible(
    fullscreen: bool,
    now: Instant,
    until: &mut Instant,
    active: bool,
    near_edge: bool,
    pinned: bool,
) -> bool {
    if !fullscreen {
        return true;
    }
    if active || near_edge || pinned {
        *until = now + Duration::from_secs(2);
    }
    pinned || near_edge || now < *until
}
impl eframe::App for PlayerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Free mpv and our FBO before eframe destroys the current GL context.
        self.renderer.lock().unwrap().take();
    }
}

#[derive(Clone, Copy)]
enum PlaylistSort {
    Name,
    NameDescending,
    Path,
    FileType,
    Newest,
    Oldest,
    Largest,
    Smallest,
    Reverse,
    Shuffle,
}

fn natural_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    while !a.is_empty() && !b.is_empty() {
        let order;
        if a[0].is_ascii_digit() && b[0].is_ascii_digit() {
            let an = a.iter().take_while(|c| c.is_ascii_digit()).count();
            let bn = b.iter().take_while(|c| c.is_ascii_digit()).count();
            let av = &a[..an];
            let bv = &b[..bn];
            let av = &av[av.iter().take_while(|c| **c == b'0').count()..];
            let bv = &bv[bv.iter().take_while(|c| **c == b'0').count()..];
            order = av.len().cmp(&bv.len()).then(av.cmp(bv));
            a = &a[an..];
            b = &b[bn..];
        } else {
            order = a[0].cmp(&b[0]);
            a = &a[1..];
            b = &b[1..];
        }
        if !order.is_eq() {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

fn folder_videos(folder: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let mut folders = vec![folder.to_path_buf()];
    let mut videos = Vec::new();
    while let Some(directory) = folders.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("Could not read {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("Could not read folder entry: {error}"))?;
            let kind = entry
                .file_type()
                .map_err(|error| format!("Could not read {}: {error}", entry.path().display()))?;
            let path = entry.path();
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "mp4"
                                | "mkv"
                                | "avi"
                                | "mov"
                                | "webm"
                                | "m4v"
                                | "mpg"
                                | "mpeg"
                                | "ts"
                                | "mts"
                                | "m2ts"
                                | "vob"
                                | "wmv"
                                | "flv"
                                | "ogv"
                                | "3gp"
                                | "3g2"
                                | "m2v"
                                | "mpe"
                                | "mxf"
                                | "divx"
                                | "f4v"
                                | "asf"
                        )
                    })
            {
                videos.push(path);
            }
        }
    }
    videos.sort_by_cached_key(|path| {
        (
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
            path.clone(),
        )
    });
    Ok(videos)
}

fn next_index(current: Option<usize>, length: usize, repeat: bool) -> Option<usize> {
    if length == 0 {
        return None;
    }
    match current {
        None => Some(0),
        Some(index) if index + 1 < length => Some(index + 1),
        Some(_) if repeat => Some(0),
        _ => None,
    }
}
fn format_time(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0) as u64
    } else {
        0
    };
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}
fn chrome_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BAR_BACKGROUND;
    visuals.window_fill = BAR_BACKGROUND;
    visuals.extreme_bg_color = Color32::from_gray(22);
    visuals.faint_bg_color = Color32::from_gray(35);
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = Color32::from_rgb(78, 53, 27);
    visuals.selection.stroke.color = ACCENT;
    visuals.widgets.inactive.bg_fill = BAR_BACKGROUND;
    visuals.widgets.inactive.weak_bg_fill = BAR_BACKGROUND;
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.fg_stroke.color = ACCENT;
        widget.corner_radius = egui::CornerRadius::same(2);
    }
    visuals
}

fn bar_style(ui: &mut egui::Ui) {
    *ui.visuals_mut() = chrome_visuals();
}

fn theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = chrome_visuals();
    style.spacing.item_spacing = egui::vec2(4.0, 3.0);
    style.spacing.button_padding = egui::vec2(5.0, 3.0);
    style.spacing.interact_size = egui::vec2(20.0, 18.0);
    style.spacing.slider_width = 80.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(11.0));
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_import_filters_sorts_and_appends_without_changing_current_media() {
        let root = std::env::temp_dir().join(format!(
            "nvplay-folder-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        for name in [
            "z.mp4",
            "A.MKV",
            "notes.txt",
            "cover.jpg",
            "nested/b.webm",
            "nested/sub.srt",
        ] {
            std::fs::write(root.join(name), b"").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("nested/loop")).unwrap();
        let paths = folder_videos(&root).unwrap();
        assert_eq!(
            paths,
            vec![
                root.join("A.MKV"),
                root.join("nested/b.webm"),
                root.join("z.mp4")
            ]
        );
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.queue.push(Media::new("existing.mp4".into()));
        app.current = Some(0);
        app.dialog_busy = true;
        app.dialog = Some(Box::pin(async move { DialogResult::Folder(Ok(paths)) }));
        app.poll_dialog();
        assert!(!app.dialog_busy);
        assert_eq!(app.current, Some(0));
        assert_eq!(
            app.queue
                .iter()
                .map(|media| media.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing.mp4", "A.MKV", "b.webm", "z.mp4"]
        );
        let empty = root.join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(folder_videos(&empty).unwrap().is_empty());
        assert!(folder_videos(&root.join("missing")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn playlist_sorts_naturally_and_preserves_current_item_with_duplicate_sources() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.queue = [
            "Episode 10.mp4",
            "episode 2.mp4",
            "Episode 1.mkv",
            "episode 2.mp4",
        ]
        .into_iter()
        .map(|source| Media::new(source.into()))
        .collect();
        app.current = Some(3);
        app.sort_playlist(PlaylistSort::Name);
        assert_eq!(
            app.queue
                .iter()
                .map(|media| media.name.as_str())
                .collect::<Vec<_>>(),
            [
                "Episode 1.mkv",
                "episode 2.mp4",
                "episode 2.mp4",
                "Episode 10.mp4"
            ]
        );
        assert_eq!(app.current, Some(2));
        app.sort_playlist(PlaylistSort::NameDescending);
        assert_eq!(app.queue.first().unwrap().name, "Episode 10.mp4");
        assert_eq!(app.current, Some(2));
        app.sort_playlist(PlaylistSort::Reverse);
        assert_eq!(app.current, Some(1));
        for sort in [
            PlaylistSort::Shuffle,
            PlaylistSort::FileType,
            PlaylistSort::Path,
            PlaylistSort::Newest,
            PlaylistSort::Oldest,
            PlaylistSort::Largest,
            PlaylistSort::Smallest,
        ] {
            app.sort_playlist(sort);
            assert_eq!(app.queue.len(), 4);
            assert_eq!(app.queue[app.current.unwrap()].source, "episode 2.mp4");
        }
        assert!(natural_compare("episode 9", "episode 100000000000000000000000000000").is_lt());
    }

    #[test]
    fn keyboard_volume_changes_accumulate_without_waiting_for_engine_events() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.initial_resize = false;
        for expected in [95.0, 90.0, 85.0] {
            draw(
                &mut app,
                vec![
                    egui::Event::Key {
                        key: Key::ArrowDown,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::Key {
                        key: Key::ArrowDown,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert_eq!(app.pending_volume.unwrap().0, expected);
        }
    }

    #[test]
    fn playlist_manual_move_keeps_the_current_media_selected() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        theme(&ctx);
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.initial_resize = false;
        app.show_playlist = true;
        app.queue = ["first.mp4", "second.mp4"]
            .into_iter()
            .map(|source| Media::new(source.into()))
            .collect();
        app.current = Some(1);
        for _ in 0..4 {
            draw(&mut app, vec![]);
        }
        let output = draw(&mut app, vec![]);
        let down = named_rect(&output, "Move down").center();
        click(&mut app, down);
        assert_eq!(app.queue[0].name, "second.mp4");
        assert_eq!(app.current, Some(0));
    }

    #[test]
    fn volume_slider_stays_in_place_across_all_percentage_widths() {
        let ctx = egui::Context::default();
        theme(&ctx);
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.initial_resize = false;
        let mut baseline = None;
        for value in [0.0, 1.0, 9.0, 10.0, 99.0, 100.0] {
            app.set_volume(value);
            draw(&mut app, vec![]);
            draw(&mut app, vec![]);
            let rect = app.control_rects.volume.unwrap();
            if let Some(baseline) = baseline {
                assert_eq!(rect, baseline, "volume {value}");
            }
            baseline = Some(rect);
        }
    }

    #[test]
    fn sidebar_toggle_reserves_video_space_and_repeat_sits_between_navigation() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        theme(&ctx);
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.initial_resize = false;
        app.queue = ["a.mp4", "b.mp4", "c.mp4"]
            .into_iter()
            .map(|source| Media::new(source.into()))
            .collect();
        app.current = Some(1);
        let video = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::Shape::Callback(callback) = &shape.shape {
                        Some(callback.rect)
                    } else {
                        None
                    }
                })
                .unwrap()
        };
        draw(&mut app, vec![]);
        let output = draw(&mut app, vec![]);
        let full = video(&output);
        let previous = named_rect(&output, "Previous");
        let repeat = named_rect(&output, "Repeat playlist");
        let next = named_rect(&output, "Next");
        assert!(previous.right() < repeat.left() && repeat.right() < next.left());
        assert_eq!(repeat.center().y, previous.center().y);
        let toggle = named_rect(&output, "Playlist sidebar").center();
        click(&mut app, toggle);
        let output = draw(&mut app, vec![]);
        assert!(app.show_playlist);
        assert!(video(&output).left() > full.left() + 190.0);
        assert_eq!(video(&output).right(), full.right());
        let close = named_rect(&output, "Close playlist sidebar").center();
        click(&mut app, close);
        let output = draw(&mut app, vec![]);
        assert!(!app.show_playlist);
        assert_eq!(video(&output), full);
    }

    #[test]
    fn preview_timestamp_is_stable_from_first_layout_and_stays_above_timeline() {
        let ctx = egui::Context::default();
        theme(&ctx);
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        let mut baseline_y = None;
        for pointer in [0.0, 320.0, 640.0, 320.0] {
            for _ in 0..2 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        app.timeline_preview(
                            ctx,
                            egui::Id::new("preview-position-test"),
                            pointer,
                            369.0,
                            None,
                            12.345,
                            0,
                        )
                    },
                );
                let timestamp = app.control_rects.preview_time.unwrap();
                assert!(timestamp.left() >= 0.0 && timestamp.right() <= 640.0);
                assert!(timestamp.bottom() < 369.0);
                if let Some(y) = baseline_y {
                    assert_eq!(timestamp.top(), y);
                }
                baseline_y = Some(timestamp.top());
            }
        }
    }

    fn draw(app: &mut PlayerApp, events: Vec<egui::Event>) -> egui::FullOutput {
        let ctx = app.ctx.clone();
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
    fn pointer(position: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    fn click(app: &mut PlayerApp, position: egui::Pos2) -> egui::FullOutput {
        draw(app, vec![egui::Event::PointerMoved(position)]);
        draw(app, vec![pointer(position, true)]);
        draw(app, vec![pointer(position, false)]);
        draw(app, Vec::new())
    }
    fn until(app: &mut PlayerApp, condition: impl Fn(&PlayerApp) -> bool) -> egui::FullOutput {
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let output = draw(app, Vec::new());
            assert!(app.error.is_none(), "{:?}", app.error);
            if condition(app) {
                return output;
            }
            assert!(
                Instant::now() < deadline,
                "UI/engine state timed out: {:?}",
                app.player.as_ref().map(|p| &p.state)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn named_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
        let nodes = &output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accessibility tree")
            .nodes;
        let (_, node) = nodes
            .iter()
            .find(|(_, node)| {
                !node.is_disabled() && node.label().is_some_and(|name| name.contains(label))
            })
            .unwrap_or_else(|| {
                panic!(
                    "Missing widget {label}: {:?}",
                    nodes
                        .iter()
                        .filter_map(|(_, n)| n.label())
                        .collect::<Vec<_>>()
                )
            });
        let bounds = node.bounds().expect("widget bounds");
        egui::Rect::from_min_max(
            egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
            egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
        )
    }
    fn slider_value(output: &egui::FullOutput, maximum: f64) -> f64 {
        output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find_map(|(_, node)| {
                if node.role() == egui::accesskit::Role::Slider
                    && node
                        .max_numeric_value()
                        .is_some_and(|v| (v - maximum).abs() < 0.1)
                {
                    node.numeric_value()
                } else {
                    None
                }
            })
            .expect("accessible slider value")
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires macOS OpenGL, libmpv and NVPLAY_TEST_MEDIA; runs UI offscreen with muted audio"]
    fn real_ui_gl_plays_at_normal_speed() {
        use eframe::glow::{self, HasContext};
        let native = crate::playback::Offscreen::new();
        let loader = |name: &std::ffi::CStr| native.address(name);
        // SAFETY: all GL calls use the current offscreen CGL context.
        let gl = Arc::new(unsafe { glow::Context::from_loader_function_cstr(loader) });
        let ctx = egui::Context::default();
        theme(&ctx);
        let player = Player::create("libmpv", Some("coreaudio"), Some(&ctx)).unwrap();
        let renderer = VideoRenderer::create(&player, gl.clone(), &loader, &ctx).unwrap();
        let media = std::env::var_os("NVPLAY_TEST_MEDIA").expect("set NVPLAY_TEST_MEDIA");
        let mut app = PlayerApp::from_parts(&ctx, Some(player), Some(renderer), None, vec![media]);
        app.initial_resize = false;
        let mut painter = eframe::egui_glow::Painter::new(gl.clone(), "", None, false).unwrap();
        let destination = unsafe {
            let texture = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                1280,
                800,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            let fbo = gl.create_framebuffer().unwrap();
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE
            );
            (fbo, texture)
        };
        let started = Instant::now();
        let mut frame_count = 0;
        let mut first = None;
        let mut max_paint_ms = 0.0_f64;
        let mut observed_duration = 0.0_f64;
        loop {
            let frame_start = Instant::now();
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 400.0),
                )),
                time: Some(started.elapsed().as_secs_f64()),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(2.0);
            let output = ctx.run(input, |ctx| app.ui(ctx));
            let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
            painter.clear([1280, 800], [0.0, 0.0, 0.0, 1.0]);
            painter.paint_and_update_textures(
                [1280, 800],
                output.pixels_per_point,
                &primitives,
                &output.textures_delta,
            );
            unsafe {
                gl.finish();
            }
            max_paint_ms = max_paint_ms.max(frame_start.elapsed().as_secs_f64() * 1000.0);
            frame_count += 1;
            assert!(app.error.is_none(), "{:?}", app.error);
            let state = &app.player.as_ref().unwrap().state;
            observed_duration = observed_duration.max(state.duration);
            if first.is_none() && state.position >= 0.1 {
                first = Some((Instant::now(), state.position));
            }
            if let Some((wall_start, media_start)) = first {
                let completed = state.idle && observed_duration > 0.0;
                let media_elapsed = if completed {
                    observed_duration
                } else {
                    state.position
                } - media_start;
                if media_elapsed >= 2.5 || completed {
                    let wall_elapsed = wall_start.elapsed().as_secs_f64();
                    let ratio = media_elapsed / wall_elapsed;
                    eprintln!(
                        "Full UI + GPU + muted CoreAudio: {media_elapsed:.2}s media in {wall_elapsed:.2}s wall; ratio {ratio:.3}, speed {:.2}, {frame_count} UI frames, max paint {max_paint_ms:.2}ms, drops {}/{}",
                        state.speed, state.dropped_frames, state.decoder_dropped_frames
                    );
                    let tolerance = if completed { 0.15 } else { 0.1 };
                    assert!(
                        (1.0 - tolerance..=1.0 + tolerance).contains(&ratio),
                        "UI playback is not running at real time"
                    );
                    assert!((state.speed - 1.0).abs() < 0.01);
                    break;
                }
            }
            assert!(
                started.elapsed() < Duration::from_secs(12),
                "UI playback fell behind real time: {:?}",
                app.player.as_ref().unwrap().state
            );
            std::thread::sleep(
                Duration::from_secs_f64(1.0 / 60.0).saturating_sub(frame_start.elapsed()),
            );
        }
        app.renderer.lock().unwrap().take();
        drop(app);
        painter.destroy();
        unsafe {
            gl.delete_framebuffer(destination.0);
            gl.delete_texture(destination.1);
        }
    }

    #[test]
    fn timeline_repaints_do_not_seek_fractional_frame_positions() {
        let ctx = egui::Context::default();
        theme(&ctx);
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.initial_resize = false;
        for position in [1.0 / 24.0, 1.0 / 30.0, 1.0 / 60.0, 0.999_999] {
            app.seek_preview = Some(position);
            draw(&mut app, vec![]);
            assert!(
                app.pending_seek.is_none(),
                "painting {position} issued an unsolicited seek"
            );
            assert_eq!(app.seek_preview, Some(position));
        }
    }

    #[test]
    fn compact_controls_fit_one_row_at_minimum_and_wide_sizes() {
        for width in [440.0, 640.0, 1280.0] {
            for fullscreen in [false, true] {
                let ctx = egui::Context::default();
                theme(&ctx);
                let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
                app.initial_resize = false;
                app.fullscreen = fullscreen;
                app.loading = true;
                app.seek_preview = Some(36_000.0);
                for _ in 0..2 {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 400.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| app.ui(ctx),
                    );
                }
                let rects = &app.control_rects;
                let play = rects.play.unwrap();
                let backward = rects.skip_backward.unwrap();
                let forward = rects.skip_forward.unwrap();
                let seek = rects.seek.unwrap();
                let stop = rects.stop.unwrap();
                let mute = rects.mute.unwrap();
                let volume = rects.volume.unwrap();
                for rect in [backward, play, forward, seek, mute, volume] {
                    assert!(
                        (rect.center().y - play.center().y).abs() < 1.0,
                        "controls wrapped at {width}, fullscreen={fullscreen}: {rect:?}"
                    );
                    assert!(
                        rect.left() >= 0.0 && rect.right() <= width,
                        "control clipped at {width}: {rect:?}"
                    );
                    assert!(rect.bottom() <= 400.0);
                }
                assert!(backward.right() < play.left());
                assert!(play.right() < forward.left());
                assert!(forward.right() < seek.left());
                assert!(seek.right() < mute.left());
                assert!(stop.bottom() < play.top());
                assert!(stop.left() > width - 106.0);
                assert!(stop.right() <= width);
                assert!(mute.right() < volume.left());
                assert_eq!(play.height(), 28.0);
                assert_eq!(play.width(), play.height());
                assert_eq!(seek.height(), play.height());
            }
        }
    }

    #[test]
    fn fullscreen_menus_and_dragging_keep_controls_visible() {
        let start = Instant::now();
        let mut until = start;
        assert!(chrome_visible(true, start, &mut until, true, false, false));
        assert!(chrome_visible(
            true,
            start + Duration::from_secs(5),
            &mut until,
            false,
            false,
            true
        ));
        assert!(chrome_visible(
            true,
            start + Duration::from_secs(6),
            &mut until,
            false,
            false,
            false
        ));
        assert!(!chrome_visible(
            true,
            start + Duration::from_secs(8),
            &mut until,
            false,
            false,
            false
        ));
        assert!(chrome_visible(
            true,
            start + Duration::from_secs(9),
            &mut until,
            false,
            true,
            false
        ));
        assert!(chrome_visible(
            false,
            start + Duration::from_secs(99),
            &mut until,
            false,
            false,
            false
        ));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn fullscreen_popup_survives_pointer_leaving_bar_without_resizing_video() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        theme(&ctx);
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.initial_resize = false;
        app.fullscreen = true;
        let output = draw(
            &mut app,
            vec![egui::Event::PointerMoved(egui::pos2(120.0, 8.0))],
        );
        let video_rect = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .find_map(|s| {
                    if let egui::Shape::Callback(callback) = &s.shape {
                        Some(callback.rect)
                    } else {
                        None
                    }
                })
                .unwrap()
        };
        let video_before = video_rect(&output);
        let output = draw(&mut app, vec![]);
        let audio = named_rect(&output, "Audio").center();
        click(&mut app, audio);
        draw(
            &mut app,
            vec![egui::Event::PointerMoved(egui::pos2(300.0, 200.0))],
        );
        app.chrome_until = Instant::now() - Duration::from_secs(10);
        let output = draw(&mut app, vec![]);
        named_rect(&output, "Audio Track");
        assert_eq!(
            video_rect(&output),
            video_before,
            "fullscreen bars resized the video"
        );
    }

    #[test]
    #[ignore = "requires libmpv, ffmpeg and NVPLAY_TEST_MEDIA (30s clip with English/French audio and subtitles)"]
    fn real_ui_pointer_controls_update_engine_and_accessible_values() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        theme(&ctx);
        let player = Player::for_ui_test(&ctx).unwrap();
        let media = std::env::var_os("NVPLAY_TEST_MEDIA").expect("set NVPLAY_TEST_MEDIA");
        let mut app = PlayerApp::from_parts(&ctx, Some(player), None, None, vec![media]);
        app.initial_resize = false;
        until(&mut app, |app| {
            !app.loading
                && app
                    .player
                    .as_ref()
                    .is_some_and(|p| p.state.seekable && p.state.tracks.len() >= 4)
        });
        let duration = app.player.as_ref().unwrap().state.duration;
        let first = slider_value(&draw(&mut app, vec![]), duration);
        let output = until(&mut app, |app| {
            app.player.as_ref().unwrap().state.position > first + 0.25
        });
        assert!(
            slider_value(&output, duration) > first + 0.2,
            "timeline did not advance"
        );

        let position = app.control_rects.play.unwrap().center();
        click(&mut app, position);
        until(&mut app, |app| app.player.as_ref().unwrap().state.paused);
        let seek = app.control_rects.seek.unwrap();
        let paused_position = app.player.as_ref().unwrap().state.position;
        let hover = egui::pos2(seek.left() + seek.width() * 0.4, seek.center().y);
        draw(&mut app, vec![egui::Event::PointerMoved(hover)]);
        until(&mut app, |app| app.frame_preview.has_frame());
        assert!(
            (app.player.as_ref().unwrap().state.position - paused_position).abs() < 0.1,
            "hover preview moved playback"
        );
        draw(
            &mut app,
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -40.0),
                modifiers: egui::Modifiers::NONE,
            }],
        );
        until(&mut app, |app| {
            app.player.as_ref().unwrap().state.position > paused_position + 0.5
        });
        assert!(
            app.player.as_ref().unwrap().state.paused,
            "scroll seeking resumed playback"
        );
        let from = egui::pos2(seek.left() + 15.0, seek.center().y);
        let to = egui::pos2(seek.left() + seek.width() * 0.6, seek.center().y);
        draw(
            &mut app,
            vec![egui::Event::PointerMoved(from), pointer(from, true)],
        );
        draw(&mut app, vec![egui::Event::PointerMoved(to)]);
        draw(&mut app, vec![pointer(to, false)]);
        let output = until(&mut app, |app| {
            app.player.as_ref().unwrap().state.position > duration * 0.5
        });
        assert!(
            (slider_value(&output, duration) - duration * 0.6).abs() < 1.0,
            "drag did not seek"
        );
        assert!(
            app.player.as_ref().unwrap().state.paused,
            "seeking resumed a paused video"
        );

        let volume = app.control_rects.volume.unwrap();
        click(
            &mut app,
            egui::pos2(volume.left() + volume.width() * 0.25, volume.center().y),
        );
        let output = until(&mut app, |app| {
            app.player.as_ref().unwrap().state.volume < 40.0
        });
        assert!(
            (slider_value(&output, 100.0) - app.player.as_ref().unwrap().state.volume).abs() < 1.0
        );
        let mute = app.control_rects.mute.unwrap().center();
        click(&mut app, mute);
        let output = until(&mut app, |app| app.player.as_ref().unwrap().state.muted);

        #[cfg(not(target_os = "macos"))]
        {
            let audio = named_rect(&output, "Audio").center();
            let output = click(&mut app, audio);
            let submenu = named_rect(&output, "Audio Track").center();
            let output = click(&mut app, submenu);
            let french = named_rect(&output, "French").center();
            click(&mut app, french);
        }
        #[cfg(target_os = "macos")]
        {
            let _ = output;
            app.activate_test_menu(&["Audio Track", "French"]);
        }
        until(&mut app, |app| {
            app.player
                .as_ref()
                .unwrap()
                .state
                .tracks
                .iter()
                .any(|t| t.title == "French" && t.selected)
        });
        assert!(!app.player.as_ref().unwrap().state.audio_devices.is_empty());
        let play = app.control_rects.play.unwrap().center();
        click(&mut app, play);
        until(&mut app, |app| {
            app.player.as_ref().is_some_and(|p| {
                !p.state.paused && p.state.audio_channels > 0 && !p.state.audio_codec.is_empty()
            })
        });

        #[cfg(not(target_os = "macos"))]
        {
            let output = draw(&mut app, vec![]);
            let media = named_rect(&output, "Media").center();
            let output = click(&mut app, media);
            let open_stream = named_rect(&output, "Open Network Stream").center();
            click(&mut app, open_stream);
        }
        #[cfg(target_os = "macos")]
        app.activate_test_menu(&["Open Network Stream"]);
        assert!(app.show_url, "Media menu did not open the URL dialog");
        app.show_url = false;
        let stop = app.control_rects.stop.unwrap().center();
        click(&mut app, stop);
        let output = until(&mut app, |app| {
            app.player.as_ref().unwrap().state.idle
                && app.player.as_ref().unwrap().state.duration == 0.0
        });
        assert_eq!(slider_value(&output, 1.0), 0.0);
    }
    #[test]
    fn queue_boundaries_and_repeat() {
        assert_eq!(next_index(None, 0, true), None);
        assert_eq!(next_index(None, 3, false), Some(0));
        assert_eq!(next_index(Some(0), 3, false), Some(1));
        assert_eq!(next_index(Some(2), 3, false), None);
        assert_eq!(next_index(Some(2), 3, true), Some(0));
        assert_eq!(next_index(Some(0), 1, true), Some(0));
    }
    #[test]
    fn media_times_are_safe_and_readable() {
        assert_eq!(format_time(65.9), "01:05");
        assert_eq!(format_time(3661.0), "1:01:01");
        assert_eq!(format_time(-1.0), "00:00");
        assert_eq!(format_time(f64::NAN), "00:00");
        assert_eq!(format_time(f64::INFINITY), "00:00");
    }
}
