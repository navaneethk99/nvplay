//! Persistent shortcuts, shared by the dispatcher, settings, help, and menus.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Settings,
    AudioSettings,
    Open,
    Network,
    SearchMovies,
    Playlist,
    Pause,
    SeekBackward,
    SeekForward,
    GoToTimestamp,
    Fullscreen,
    LeaveFullscreen,
    Mute,
    VolumeUp,
    VolumeDown,
    Stop,
    Previous,
    Next,
    Repeat,
    History,
    Info,
    AddSubtitle,
    DownloadSubtitle,
    SubtitleAppearance,
    ToggleSubtitles,
    Help,
}

impl Action {
    pub const ALL: [Self; 26] = [
        Self::Settings,
        Self::AudioSettings,
        Self::Open,
        Self::Network,
        Self::SearchMovies,
        Self::Playlist,
        Self::Pause,
        Self::SeekBackward,
        Self::SeekForward,
        Self::GoToTimestamp,
        Self::Fullscreen,
        Self::LeaveFullscreen,
        Self::Mute,
        Self::VolumeUp,
        Self::VolumeDown,
        Self::Stop,
        Self::Previous,
        Self::Next,
        Self::Repeat,
        Self::History,
        Self::Info,
        Self::AddSubtitle,
        Self::DownloadSubtitle,
        Self::SubtitleAppearance,
        Self::ToggleSubtitles,
        Self::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::AudioSettings => "Audio settings",
            Self::Open => "Open media",
            Self::Network => "Open network stream",
            Self::SearchMovies => "Search movies",
            Self::Playlist => "Toggle playlist",
            Self::Pause => "Play / pause",
            Self::SeekBackward => "Seek backward 5 seconds",
            Self::SeekForward => "Seek forward 5 seconds",
            Self::GoToTimestamp => "Go to timestamp",
            Self::Fullscreen => "Toggle fullscreen",
            Self::LeaveFullscreen => "Leave fullscreen",
            Self::Mute => "Mute / unmute",
            Self::VolumeUp => "Volume up",
            Self::VolumeDown => "Volume down",
            Self::Stop => "Stop",
            Self::Previous => "Previous media",
            Self::Next => "Next media",
            Self::Repeat => "Repeat playlist",
            Self::History => "History",
            Self::Info => "Playback information",
            Self::AddSubtitle => "Add subtitle file",
            Self::DownloadSubtitle => "Download subtitles",
            Self::SubtitleAppearance => "Subtitle appearance",
            Self::ToggleSubtitles => "Show / hide subtitles",
            Self::Help => "Keyboard shortcuts",
        }
    }

    fn default_binding(self) -> Option<Binding> {
        let (key, command, shift) = match self {
            Self::Settings => (Key::Comma, true, false),
            Self::Open => (Key::O, true, false),
            Self::Network => (Key::N, true, false),
            Self::SearchMovies => (Key::M, true, true),
            Self::Playlist => (Key::L, true, false),
            Self::Pause => (Key::Space, false, false),
            Self::SeekBackward => (Key::ArrowLeft, false, false),
            Self::SeekForward => (Key::ArrowRight, false, false),
            Self::Fullscreen => (Key::F, false, false),
            Self::LeaveFullscreen => (Key::Escape, false, false),
            Self::Mute => (Key::M, false, false),
            Self::VolumeUp => (Key::ArrowUp, false, false),
            Self::VolumeDown => (Key::ArrowDown, false, false),
            _ => return None,
        };
        Some(Binding {
            key: key.name().into(),
            command,
            shift,
            ctrl: false,
            alt: false,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    key: String,
    #[serde(default)]
    command: bool,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    shift: bool,
}
impl Binding {
    fn from_event(key: Key, modifiers: egui::Modifiers) -> Self {
        Self {
            key: key.name().into(),
            command: modifiers.command
                || modifiers.mac_cmd
                || (!cfg!(target_os = "macos") && modifiers.ctrl),
            ctrl: cfg!(target_os = "macos")
                && modifiers.ctrl
                && (!modifiers.command || modifiers.mac_cmd),
            alt: modifiers.alt,
            shift: modifiers.shift,
        }
    }
    fn validate(&self, action: Action) -> Result<(), String> {
        if Key::from_name(&self.key).is_none() {
            return Err(format!("Unknown key for {}.", action.label()));
        }
        if self.key == "Escape"
            && !self.command
            && !self.ctrl
            && !self.alt
            && !self.shift
            && action != Action::LeaveFullscreen
        {
            return Err("Escape is reserved for closing dialogs and leaving fullscreen.".into());
        }
        if cfg!(target_os = "macos")
            && self.command
            && !self.ctrl
            && matches!(self.key.as_str(), "Q" | "H" | "M" | "W")
            && !self.shift
        {
            return Err(
                "This shortcut is used by the macOS window menu. Choose another combination."
                    .into(),
            );
        }
        Ok(())
    }
    fn label(&self, ctx: &egui::Context) -> String {
        let modifiers = egui::Modifiers {
            command: self.command,
            ctrl: self.ctrl || (self.command && !cfg!(target_os = "macos")),
            mac_cmd: self.command && cfg!(target_os = "macos"),
            alt: self.alt,
            shift: self.shift,
        };
        egui::KeyboardShortcut::new(modifiers, Key::from_name(&self.key).unwrap())
            .format(&egui::ModifierNames::NAMES, ctx.os().is_mac())
    }
}

#[derive(Default)]
pub(super) struct KeyBindings {
    // Only overrides are stored; missing entries inherit new defaults.
    overrides: BTreeMap<Action, Option<Binding>>,
    pub capturing: Option<Action>,
    pub error: Option<String>,
    #[cfg(test)]
    save_path: Option<PathBuf>,
}
impl KeyBindings {
    pub fn load() -> Self {
        let result = crate::settings::directory().and_then(|dir| {
            match std::fs::read(dir.join("key-bindings.json")) {
                Ok(bytes) => Self::decode(&bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
                Err(e) => Err(format!("Could not read key bindings: {e}")),
            }
        });
        match result {
            Ok(overrides) => Self {
                overrides,
                ..Self::default()
            },
            Err(error) => Self {
                error: Some(error),
                ..Self::default()
            },
        }
    }
    fn decode(bytes: &[u8]) -> Result<BTreeMap<Action, Option<Binding>>, String> {
        let overrides: BTreeMap<Action, Option<Binding>> = serde_json::from_slice(bytes)
            .map_err(|e| format!("Could not read key bindings: {e}"))?;
        let bindings = Self {
            overrides: overrides.clone(),
            ..Self::default()
        };
        for (index, action) in Action::ALL.iter().enumerate() {
            if let Some(binding) = bindings.binding(*action) {
                binding.validate(*action)?;
                if Action::ALL[..index]
                    .iter()
                    .any(|other| bindings.binding(*other).as_ref() == Some(&binding))
                {
                    return Err("Saved key bindings contain duplicate shortcuts.".into());
                }
            }
        }
        Ok(overrides)
    }
    fn binding(&self, action: Action) -> Option<Binding> {
        self.overrides
            .get(&action)
            .cloned()
            .unwrap_or_else(|| action.default_binding())
    }
    pub fn label(&self, action: Action, ctx: &egui::Context) -> String {
        self.binding(action)
            .map_or_else(|| "Unassigned".into(), |b| b.label(ctx))
    }
    fn save(&mut self, overrides: BTreeMap<Action, Option<Binding>>) {
        let path = crate::settings::directory().map(|dir| dir.join("key-bindings.json"));
        #[cfg(test)]
        let path = self.save_path.clone().map(Ok).unwrap_or(path);
        let result = path.and_then(|path| {
            let bytes = serde_json::to_vec_pretty(&overrides).map_err(|e| e.to_string())?;
            crate::settings::write_private(&path, &bytes)
                .map_err(|e| format!("Could not save key bindings: {e}"))
        });
        match result {
            Ok(()) => {
                self.overrides = overrides;
                self.error = None;
                self.capturing = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn conflict(&self, action: Action, binding: &Binding) -> Option<Action> {
        Action::ALL
            .into_iter()
            .find(|other| *other != action && self.binding(*other).as_ref() == Some(binding))
    }
    pub fn hint(&self, action: Action, title: &str, ctx: &egui::Context) -> String {
        self.binding(action).map_or_else(
            || title.to_owned(),
            |binding| format!("{title} ({})", binding.label(ctx)),
        )
    }
    pub fn global_pressed(&self, ctx: &egui::Context, action: Action) -> bool {
        if ctx.wants_keyboard_input()
            && self
                .binding(action)
                .is_some_and(|b| !b.command && !b.ctrl && !b.alt)
        {
            return false;
        }
        self.pressed(ctx, action)
    }
    pub fn pressed(&self, ctx: &egui::Context, action: Action) -> bool {
        let Some(binding) = self.binding(action) else {
            return false;
        };
        // Exact modifiers prevent shifted shortcuts also triggering their unshifted action.
        ctx.input_mut(|input| {
            let mut pressed = false;
            input.events.retain(|event| {
                if let egui::Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    ..
                } = event
                    && Binding::from_event(*key, *modifiers) == binding
                {
                    pressed = true;
                    return false;
                }
                true
            });
            pressed
        })
    }
}

impl PlayerApp {
    pub(super) fn capture_key_binding(&mut self, ctx: &egui::Context) {
        let Some(action) = self.key_bindings.capturing else {
            return;
        };
        let event = ctx.input(|input| {
            input.events.iter().find_map(|event| {
                if let egui::Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    repeat: false,
                    ..
                } = event
                {
                    Some((*key, *modifiers))
                } else {
                    None
                }
            })
        });
        // No captured key may activate a dialog, menu, or playback control.
        ctx.input_mut(|input| {
            input.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Paste(_)
                )
            })
        });
        if let Some((key, modifiers)) = event {
            let binding = Binding::from_event(key, modifiers);
            if let Err(error) = binding.validate(action) {
                self.key_bindings.error = Some(error);
            } else if let Some(other) = self.key_bindings.conflict(action, &binding) {
                self.key_bindings.error = Some(format!(
                    "Already assigned to {}. Clear or change that shortcut first.",
                    other.label()
                ));
            } else {
                let mut overrides = self.key_bindings.overrides.clone();
                overrides.insert(action, Some(binding));
                self.key_bindings.save(overrides);
            }
        }
    }

    pub(super) fn key_bindings_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Key Bindings");
        ui.label("Click a shortcut, then press a key or key combination. Changes are saved automatically.");
        ui.small("Escape always closes dialogs. Double-click video also toggles fullscreen.");
        if self.key_bindings.capturing.is_some() {
            ui.label("Press your new shortcut (including Escape), or click Cancel.");
            if ui.button("Cancel").clicked() {
                self.key_bindings.capturing = None;
                self.key_bindings.error = None;
            }
        }
        if let Some(error) = &self.key_bindings.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.add_space(8.0);
        egui::Grid::new("key_bindings")
            .num_columns(3)
            .min_col_width(0.0)
            .max_col_width(((ui.available_width() - 24.0) / 3.0).max(60.0))
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for action in Action::ALL {
                    ui.label(action.label());
                    let capturing = self.key_bindings.capturing == Some(action);
                    let label = if capturing {
                        "Press a key…".into()
                    } else {
                        self.key_bindings.label(action, ui.ctx())
                    };
                    let shortcut = ui.add(egui::Button::new(&label).wrap());
                    shortcut.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            shortcut.enabled(),
                            format!("{} shortcut: {label}", action.label()),
                        )
                    });
                    if shortcut
                        .on_hover_text(format!("Change shortcut for {}", action.label()))
                        .clicked()
                    {
                        self.key_bindings.capturing = Some(action);
                        self.key_bindings.error = None;
                    }
                    let clear = ui.add_enabled(
                        self.key_bindings.binding(action).is_some(),
                        egui::Button::new("Clear"),
                    );
                    clear.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            clear.enabled(),
                            format!("Clear {} shortcut", action.label()),
                        )
                    });
                    if clear
                        .on_hover_text(format!("Remove shortcut for {}", action.label()))
                        .clicked()
                    {
                        let mut overrides = self.key_bindings.overrides.clone();
                        overrides.insert(action, None);
                        self.key_bindings.save(overrides);
                    }
                    ui.end_row();
                }
            });
        ui.separator();
        if ui.button("Reset all to defaults").clicked() {
            self.key_bindings.save(BTreeMap::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    fn frame(app: &mut PlayerApp, events: Vec<egui::Event>) {
        let ctx = app.ctx.clone();
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
    }
    fn temporary_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "nvplay-keys-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .join("key-bindings.json")
    }

    #[test]
    fn key_bindings_defaults_are_valid_unique_and_round_trip() {
        assert!(KeyBindings::decode(b"{}").is_ok());
        for action in Action::ALL {
            let mut bindings = KeyBindings::default();
            bindings.overrides.insert(action, None);
            let bytes = serde_json::to_vec(&bindings.overrides).unwrap();
            let decoded = KeyBindings::decode(&bytes).unwrap();
            assert_eq!(decoded.get(&action), Some(&None));
        }
        assert!(KeyBindings::decode(br#"{"pause":{"key":"Unknown"}}"#).is_err());
        assert!(KeyBindings::decode(br#"{"pause":{"key":"M"}}"#).is_err());
        assert!(KeyBindings::decode(br#"{"settings":{"key":"Escape"}}"#).is_err());
        assert!(KeyBindings::decode(b"broken json").is_err());
    }

    #[test]
    fn key_bindings_capture_saves_and_replaces_default_without_triggering_actions() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        let path = temporary_path();
        app.key_bindings = KeyBindings {
            save_path: Some(path.clone()),
            ..Default::default()
        };
        app.show_settings = true;
        app.settings_tab = settings_panel::SettingsTab::KeyBindings;
        app.key_bindings.capturing = Some(Action::Playlist);
        frame(&mut app, vec![key(Key::P, egui::Modifiers::SHIFT)]);
        assert!(!app.show_playlist);
        assert!(app.key_bindings.capturing.is_none());
        assert!(app.key_bindings.error.is_none());
        let restored = KeyBindings::decode(&std::fs::read(&path).unwrap()).unwrap();
        app.key_bindings.overrides = restored;
        app.show_settings = false;
        frame(&mut app, vec![key(Key::L, egui::Modifiers::COMMAND)]);
        assert!(!app.show_playlist, "old shortcut must stop working");
        frame(&mut app, vec![key(Key::P, egui::Modifiers::NONE)]);
        assert!(!app.show_playlist, "modifiers must match exactly");
        frame(&mut app, vec![key(Key::P, egui::Modifiers::SHIFT)]);
        assert!(app.show_playlist);
        let mut cleared = app.key_bindings.overrides.clone();
        cleared.insert(Action::Playlist, None);
        app.key_bindings.save(cleared);
        assert!(app.key_bindings.binding(Action::Playlist).is_none());
        assert_eq!(
            KeyBindings::decode(&std::fs::read(&path).unwrap())
                .unwrap()
                .get(&Action::Playlist),
            Some(&None)
        );
        app.key_bindings.save(BTreeMap::new());
        assert_eq!(
            app.key_bindings.binding(Action::Playlist),
            Action::Playlist.default_binding()
        );
        assert!(
            KeyBindings::decode(&std::fs::read(&path).unwrap())
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn key_bindings_conflicts_keep_capture_open_and_do_not_close_settings() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.key_bindings = KeyBindings::default();
        app.show_settings = true;
        app.settings_tab = settings_panel::SettingsTab::KeyBindings;
        app.key_bindings.capturing = Some(Action::Pause);
        frame(&mut app, vec![key(Key::M, egui::Modifiers::NONE)]);
        assert!(app.key_bindings.error.as_ref().unwrap().contains("Mute"));
        assert_eq!(app.key_bindings.capturing, Some(Action::Pause));
        frame(&mut app, vec![key(Key::Escape, egui::Modifiers::NONE)]);
        assert!(app.show_settings);
        assert!(
            app.key_bindings
                .error
                .as_ref()
                .unwrap()
                .contains("reserved")
        );
        assert_eq!(
            app.key_bindings.binding(Action::Pause),
            Action::Pause.default_binding()
        );
        // Closing the window or changing tabs must never leave key capture active.
        app.show_settings = false;
        frame(&mut app, vec![]);
        assert!(app.key_bindings.capturing.is_none());
    }

    #[test]
    fn key_bindings_failed_save_preserves_active_shortcuts() {
        let path = temporary_path();
        std::fs::create_dir_all(&path).unwrap(); // A directory cannot be replaced with a settings file.
        let mut bindings = KeyBindings {
            save_path: Some(path.clone()),
            ..Default::default()
        };
        let mut changes = BTreeMap::new();
        changes.insert(Action::Pause, None);
        bindings.save(changes);
        assert!(bindings.error.as_ref().unwrap().contains("Could not save"));
        assert_eq!(
            bindings.binding(Action::Pause),
            Action::Pause.default_binding()
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn key_bindings_do_not_trigger_playback_or_plain_global_keys_while_typing() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.key_bindings = KeyBindings::default();
        app.key_bindings.overrides.insert(
            Action::Settings,
            Some(Binding::from_event(Key::S, egui::Modifiers::NONE)),
        );
        let id = egui::Id::new("typing_test");
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut String::new()).id(id))
                    .request_focus();
            });
        });
        frame(
            &mut app,
            vec![
                key(Key::S, egui::Modifiers::NONE),
                key(Key::L, egui::Modifiers::COMMAND),
            ],
        );
        assert!(!app.show_settings);
        assert!(!app.show_playlist);
    }
}
