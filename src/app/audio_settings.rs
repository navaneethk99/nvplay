//! Audio preferences, acknowledged engine updates, and the Audio settings page.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const FREQUENCIES: [u32; 10] = [31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
const LABELS: [&str; 10] = [
    "31 Hz", "63 Hz", "125 Hz", "250 Hz", "500 Hz", "1 kHz", "2 kHz", "4 kHz", "8 kHz", "16 kHz",
];
const PRESETS: [(&str, [f64; 10]); 6] = [
    ("Flat", [0.0; 10]),
    (
        "Bass boost",
        [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        "Clear dialogue",
        [-3.0, -2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 2.0, 0.0, -1.0],
    ),
    ("Rock", [4.0, 3.0, 1.0, -1.0, -2.0, 0.0, 2.0, 3.0, 4.0, 4.0]),
    (
        "Classical",
        [3.0, 2.0, 0.0, 0.0, -1.0, -1.0, 0.0, 2.0, 3.0, 3.0],
    ),
    (
        "Treble cut",
        [0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0, -4.0, -6.0, -8.0],
    ),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Sound {
    enabled: bool,
    gains: [f64; 10],
    preamp: f64,
    balance: f64,
    mono: bool,
    night: bool,
    leveling: bool,
    limiter: bool,
}
impl Default for Sound {
    fn default() -> Self {
        Self {
            enabled: false,
            gains: [0.0; 10],
            preamp: 0.0,
            balance: 0.0,
            mono: false,
            night: false,
            leveling: false,
            limiter: true,
        }
    }
}
impl Sound {
    fn validate(&self) -> Result<(), String> {
        let bounded =
            |value: f64, low: f64, high: f64| value.is_finite() && (low..=high).contains(&value);
        if !self.gains.iter().all(|v| bounded(*v, -12.0, 12.0))
            || !bounded(self.preamp, -24.0, 12.0)
            || !bounded(self.balance, -1.0, 1.0)
        {
            return Err(
                "Saved audio values are outside the supported range. Using defaults.".into(),
            );
        }
        Ok(())
    }
    fn read(path: &std::path::Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let sound: Self = serde_json::from_slice(&bytes)
                    .map_err(|_| "Could not read audio settings. Using defaults.".to_string())?;
                sound.validate()?;
                Ok(sound)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err("Could not read audio settings. Using defaults.".into()),
        }
    }
    fn write(&self, path: &std::path::Path) -> Result<(), String> {
        self.validate()?;
        crate::settings::write_private(
            path,
            &serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|_| "Audio changes are active, but could not be saved. Try saving again.".into())
    }
    fn filter(&self) -> String {
        if !self.enabled {
            return "@nvplay-audio:lavfi=[anull]".into();
        }
        // Reserve headroom for the strongest boosted band before equalization.
        let boost = self.gains.iter().copied().fold(0.0_f64, f64::max);
        let mut filters = vec![format!(
            "volume={:.6}",
            10_f64.powf((self.preamp - boost) / 20.0)
        )];
        for (frequency, gain) in FREQUENCIES.into_iter().zip(self.gains) {
            if gain.abs() >= 0.01 {
                filters.push(format!("equalizer=f={frequency}:t=o:w=1:g={gain:.2}"));
            }
        }
        if self.mono || self.balance.abs() > 0.001 {
            filters.push("aformat=channel_layouts=stereo".into());
            if self.mono {
                filters.push("pan=mono|c0=0.5*c0+0.5*c1".into());
            } else {
                let left = 1.0 - self.balance.max(0.0);
                let right = 1.0 + self.balance.min(0.0);
                filters.push(format!("pan=stereo|c0={left:.4}*c0|c1={right:.4}*c1"));
            }
        }
        if self.leveling {
            filters.push("dynaudnorm=f=150:g=5:p=0.9:m=3:r=0.15".into());
        }
        if self.night {
            filters
                .push("acompressor=threshold=0.125:ratio=4:attack=10:release=250:makeup=2".into());
        }
        if self.limiter {
            filters.push("alimiter=limit=0.98:level=false:latency=true".into());
        }
        format!("@nvplay-audio:lavfi=[{}]", filters.join(","))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct MixInput {
    id: i64,
    level: f64,
}
fn mix_graph(inputs: &[MixInput]) -> Result<String, String> {
    if inputs.len() < 2 || inputs.len() > 4 || !inputs.iter().any(|t| t.level > 0.0) {
        return Err("Select two to four tracks with at least one level above zero.".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    if inputs.iter().any(|t| {
        t.id <= 0 || !ids.insert(t.id) || !t.level.is_finite() || !(0.0..=1.0).contains(&t.level)
    }) {
        return Err("Invalid audio track or mixer level.".into());
    }
    let labels = inputs
        .iter()
        .map(|t| format!("[aid{}]", t.id))
        .collect::<String>();
    let weights = inputs
        .iter()
        .map(|t| format!("{:.3}", t.level))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!(
        "{labels}amix=inputs={}:weights='{weights}':normalize=1:duration=longest:dropout_transition=0[ao]",
        inputs.len()
    ))
}

enum Pending {
    Sound(Sound),
    Mix(Vec<MixInput>),
    Other,
    Recovery,
    Delay,
}
pub(super) struct AudioSettings {
    sound: Sound,
    applied: Sound,
    save_candidate: Sound,
    requested_delay: Option<f64>,
    pending: BTreeMap<u64, Pending>,
    next_id: u64,
    apply_after: Option<Instant>,
    save_after: Option<Instant>,
    save_error: Option<String>,
    engine_error: Option<String>,
    mix: Vec<MixInput>,
    applied_mix: Vec<MixInput>,
    single_track: String,
}
impl Default for AudioSettings {
    fn default() -> Self {
        #[cfg(not(test))]
        let loaded = crate::settings::directory().and_then(|p| Sound::read(&p.join("audio.json")));
        #[cfg(test)]
        let loaded: Result<Sound, String> = Ok(Sound::default());
        let save_error = loaded.as_ref().err().cloned();
        Self {
            sound: loaded.clone().unwrap_or_default(),
            save_candidate: loaded.unwrap_or_default(),
            requested_delay: None,
            applied: Sound::default(),
            pending: BTreeMap::new(),
            next_id: crate::playback::AUDIO_SETTINGS_TAG,
            apply_after: None,
            save_after: None,
            save_error,
            engine_error: None,
            mix: vec![],
            applied_mix: vec![],
            single_track: "auto".into(),
        }
    }
}
impl AudioSettings {
    pub fn flush(&mut self) {
        if self.save_after.take().is_none() {
            return;
        }
        #[cfg(not(test))]
        {
            self.save_error = crate::settings::directory()
                .and_then(|p| self.save_candidate.write(&p.join("audio.json")))
                .err();
        }
    }
    fn has_sound_pending(&self) -> bool {
        self.pending
            .values()
            .any(|p| matches!(p, Pending::Sound(_)))
    }
    fn has_mix_pending(&self) -> bool {
        self.pending.values().any(|p| matches!(p, Pending::Mix(_)))
    }
}

impl PlayerApp {
    fn audio_request(&mut self, args: &[&str], pending: Pending) {
        let id = self.audio_settings.next_id;
        self.audio_settings.next_id = ((id.wrapping_add(1))
            & (crate::playback::AUDIO_SETTINGS_TAG - 1))
            | crate::playback::AUDIO_SETTINGS_TAG;
        if let Some(player) = &mut self.player {
            match player.configure_audio(args, id) {
                Ok(()) => {
                    self.audio_settings.pending.insert(id, pending);
                }
                Err(e) => {
                    self.audio_settings.engine_error =
                        Some(format!("Could not apply audio settings: {e}"));
                }
            }
        }
    }
    pub(super) fn audio_settings_applied(&mut self, id: u64, result: Result<(), String>) {
        let Some(pending) = self.audio_settings.pending.remove(&id) else {
            return;
        };
        match result {
            Ok(()) => {
                if !matches!(pending, Pending::Recovery) {
                    self.audio_settings.engine_error = None;
                }
                match pending {
                    Pending::Sound(sound) => {
                        self.audio_settings.applied = sound.clone();
                        self.audio_settings.save_candidate = sound;
                        self.audio_settings.save_after =
                            Some(Instant::now() + Duration::from_millis(400));
                    }
                    Pending::Mix(inputs) => {
                        self.audio_settings.applied_mix = inputs;
                    }
                    Pending::Other | Pending::Recovery | Pending::Delay => {}
                }
            }
            Err(error) => {
                match pending {
                    Pending::Sound(_) => {
                        self.audio_settings.sound = self.audio_settings.applied.clone();
                        self.audio_settings.apply_after = None;
                        self.audio_settings.save_candidate = self.audio_settings.applied.clone();
                        self.audio_settings.save_after = Some(Instant::now());
                        let filter = self.audio_settings.applied.filter();
                        self.audio_request(&["af", "add", &filter], Pending::Recovery);
                    }
                    Pending::Mix(_) => {
                        self.audio_settings.mix = self.audio_settings.applied_mix.clone();
                        let graph = if self.audio_settings.applied_mix.is_empty() {
                            String::new()
                        } else {
                            mix_graph(&self.audio_settings.applied_mix).unwrap_or_default()
                        };
                        self.audio_request(&["set", "lavfi-complex", &graph], Pending::Recovery);
                    }
                    Pending::Delay => self.audio_settings.requested_delay = None,
                    Pending::Other | Pending::Recovery => {}
                }
                self.audio_settings.engine_error =
                    Some(format!("Audio change was not applied: {error}"));
            }
        }
    }
    pub(super) fn poll_audio_settings(&mut self, ctx: &egui::Context) {
        let ready = !self.loading
            && self
                .player
                .as_ref()
                .is_some_and(|p| !p.state.idle && p.state.tracks.iter().any(|t| t.kind == "audio"));
        if self.audio_settings.requested_delay.is_some_and(|target| {
            self.player
                .as_ref()
                .is_some_and(|p| (p.state.audio_delay - target).abs() < 0.001)
        }) {
            self.audio_settings.requested_delay = None;
        }
        if ready
            && self
                .audio_settings
                .apply_after
                .is_some_and(|t| t <= Instant::now())
            && !self.audio_settings.has_sound_pending()
        {
            self.audio_settings.apply_after = None;
            let sound = self.audio_settings.sound.clone();
            self.audio_request(&["af", "add", &sound.filter()], Pending::Sound(sound));
        }
        if self
            .audio_settings
            .save_after
            .is_some_and(|t| t <= Instant::now())
        {
            self.audio_settings.flush();
        }
        if (ready && self.audio_settings.apply_after.is_some())
            || !self.audio_settings.pending.is_empty()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    pub(super) fn restore_audio_settings(&mut self) {
        if self.audio_settings.sound.enabled {
            self.audio_settings.apply_after = Some(Instant::now());
        }
    }
    pub(super) fn clear_audio_mix(&mut self) {
        // Track IDs belong to one file; never carry a graph into the next file.
        if !self.audio_settings.mix.is_empty()
            || !self.audio_settings.applied_mix.is_empty()
            || self.audio_settings.has_mix_pending()
        {
            self.command(&["set", "lavfi-complex", ""]);
            self.audio_settings.mix.clear();
            self.audio_settings.applied_mix.clear();
            self.audio_settings
                .pending
                .retain(|_, p| !matches!(p, Pending::Mix(_)));
        }
    }
    fn pick_external_audio(&mut self) {
        if self.dialog_busy {
            return;
        }
        self.dialog_busy = true;
        let generation = self.playback_generation;
        self.dialog = Some(Box::pin(async move {
            rfd::AsyncFileDialog::new()
                .set_title("Add external audio")
                .add_filter(
                    "Audio",
                    &[
                        "mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "ac3", "mka",
                    ],
                )
                .pick_file()
                .await
                .map(|file| DialogResult::Audio {
                    path: file.path().to_path_buf(),
                    generation,
                })
                .unwrap_or(DialogResult::Media(vec![]))
        }));
        self.ctx.request_repaint();
    }
    pub(super) fn add_external_audio(&mut self, path: PathBuf) {
        if self.loading || self.player.as_ref().is_none_or(|p| p.state.idle) {
            return;
        }
        let Some(source) = path.to_str() else {
            self.audio_settings.engine_error = Some("The audio path is not valid Unicode.".into());
            return;
        };
        let title = path.file_name().unwrap_or_default().to_string_lossy();
        self.audio_request(&["audio-add", source, "auto", &title], Pending::Other);
    }
    pub(super) fn open_audio_settings(&mut self) {
        self.show_settings = true;
        self.settings_tab = super::settings_panel::SettingsTab::Audio;
    }
    pub(super) fn audio_settings_ui(&mut self, ui: &mut egui::Ui) {
        let ready = !self.loading && self.player.as_ref().is_some_and(|p| !p.state.idle);
        let (tracks, devices, device, aid, delay, codec, channels) = self
            .player
            .as_ref()
            .map(|p| {
                (
                    p.state
                        .tracks
                        .iter()
                        .filter(|t| t.kind == "audio")
                        .cloned()
                        .collect::<Vec<_>>(),
                    p.state.audio_devices.clone(),
                    p.state.audio_device.clone(),
                    p.state.audio_track.clone(),
                    p.state.audio_delay,
                    p.state.audio_codec.clone(),
                    p.state.audio_channels,
                )
            })
            .unwrap_or_default();
        let has_audio = ready && !tracks.is_empty();
        ui.heading("Audio");
        ui.small("Sound enhancements are saved. Tracks, output, and sync apply to this session.");
        ui.add_space(8.0);
        ui.strong("Source & output");
        ui.add_enabled_ui(self.player.is_some(), |ui| {
            egui::ComboBox::from_label("Output device")
                .selected_text(
                    devices
                        .iter()
                        .find(|d| d.name == device)
                        .map_or("System default", |d| d.description.as_str()),
                )
                .width(230.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(device.is_empty() || device == "auto", "System default")
                        .clicked()
                    {
                        self.audio_request(&["set", "audio-device", "auto"], Pending::Other);
                    }
                    for d in &devices {
                        if d.name != "auto"
                            && ui
                                .selectable_label(d.name == device, &d.description)
                                .clicked()
                        {
                            self.audio_request(&["set", "audio-device", &d.name], Pending::Other);
                        }
                    }
                });
        });
        ui.small("For speakers and headphones together, select a system multi-output device here.");
        #[cfg(target_os = "macos")]
        if ui.button("Open Audio MIDI Setup…").clicked() {
            if let Err(e) = std::process::Command::new("/usr/bin/open")
                .args(["-b", "com.apple.audio.AudioMIDISetup"])
                .spawn()
            {
                self.audio_settings.engine_error =
                    Some(format!("Could not open Audio MIDI Setup: {e}"));
            }
        }
        ui.add_enabled_ui(ready && !self.audio_settings.has_mix_pending(), |ui| {
            let mixing = !self.audio_settings.applied_mix.is_empty();
            let current = if mixing {
                "Multiple tracks mixed".into()
            } else {
                tracks.iter().find(|t| t.id.to_string() == aid).map_or_else(
                    || {
                        if aid == "no" {
                            "Off".into()
                        } else {
                            "Automatic".into()
                        }
                    },
                    |t| t.label(),
                )
            };
            egui::ComboBox::from_label("Audio track")
                .selected_text(current)
                .width(230.0)
                .show_ui(ui, |ui| {
                    for (id, label) in [
                        ("auto".into(), "Automatic".into()),
                        ("no".into(), "Off".into()),
                    ]
                    .into_iter()
                    .chain(tracks.iter().map(|t| (t.id.to_string(), t.label())))
                    {
                        if ui.selectable_label(!mixing && id == aid, label).clicked() {
                            self.clear_audio_mix();
                            self.audio_request(&["set", "aid", &id], Pending::Other);
                        }
                    }
                });
        });
        if ui
            .add_enabled(
                ready && !self.dialog_busy,
                egui::Button::new("Add external audio…"),
            )
            .clicked()
        {
            self.pick_external_audio();
        }
        ui.add_enabled_ui(self.player.is_some(), |ui| {
            let mut volume = self.pending_volume.map_or_else(
                || self.player.as_ref().map_or(100.0, |p| p.state.volume),
                |p| p.0,
            );
            if ui
                .add(
                    egui::Slider::new(&mut volume, 0.0..=100.0)
                        .text("Volume")
                        .suffix("%"),
                )
                .changed()
            {
                self.set_volume(volume);
            }
            let mut muted = self.is_muted();
            if ui.checkbox(&mut muted, "Mute").changed() {
                self.set_muted(muted);
            }
        });
        if !codec.is_empty() {
            ui.small(format!("{codec} · {channels} channels"));
        }
        if !ready {
            ui.small("Open media to choose tracks, adjust sync, or mix sources.");
        }
        ui.separator();
        ui.strong("Sound enhancements");
        let before = self.audio_settings.sound.clone();
        ui.checkbox(
            &mut self.audio_settings.sound.enabled,
            "Enable sound enhancements",
        );
        let preset = PRESETS
            .iter()
            .find(|(_, gains)| gains == &self.audio_settings.sound.gains)
            .map_or("Custom", |(name, _)| *name);
        egui::ComboBox::from_label("Equalizer preset")
            .selected_text(preset)
            .show_ui(ui, |ui| {
                for (name, gains) in PRESETS {
                    if ui.selectable_label(preset == name, name).clicked() {
                        self.audio_settings.sound.gains = gains;
                        self.audio_settings.sound.enabled = true;
                    }
                }
            });
        ui.add_enabled_ui(self.audio_settings.sound.enabled, |ui| {
            let sound = &mut self.audio_settings.sound;
            egui::Grid::new("audio_eq_bands")
                .num_columns(2)
                .show(ui, |ui| {
                    for (index, label) in LABELS.into_iter().enumerate() {
                        ui.label(label);
                        ui.add(
                            egui::Slider::new(&mut sound.gains[index], -12.0..=12.0)
                                .step_by(0.5)
                                .suffix(" dB"),
                        );
                        ui.end_row();
                    }
                });
            ui.small("Bass: 31–250 Hz · Mids: 500 Hz–2 kHz · Treble: 4–16 kHz");
            ui.add(
                egui::Slider::new(&mut sound.preamp, -24.0..=12.0)
                    .text("Preamp")
                    .suffix(" dB")
                    .step_by(0.5),
            );
            ui.small("Headroom is reserved automatically when boosting EQ bands.");
            ui.add_enabled_ui(!sound.mono, |ui| {
                ui.add(
                    egui::Slider::new(&mut sound.balance, -1.0..=1.0)
                        .text("Balance")
                        .custom_formatter(|v, _| {
                            if v.abs() < 0.01 {
                                "Center".into()
                            } else {
                                format!(
                                    "{} {:.0}%",
                                    if v < 0.0 { "L" } else { "R" },
                                    v.abs() * 100.0
                                )
                            }
                        }),
                );
            });
            ui.checkbox(&mut sound.mono, "Mono downmix");
            ui.small("Balance and mono downmix surround audio before processing.");
            ui.checkbox(&mut sound.night, "Night mode — soften loud sounds");
            ui.checkbox(
                &mut sound.leveling,
                "Volume leveling — even out quiet and loud passages",
            );
            ui.checkbox(&mut sound.limiter, "Peak limiter");
        });
        if ui.button("Reset sound enhancements").clicked() {
            self.audio_settings.sound = Sound::default();
        }
        if self.audio_settings.sound != before {
            self.audio_settings.engine_error = None;
            self.audio_settings.apply_after = Some(Instant::now() + Duration::from_millis(180));
            if !has_audio {
                self.audio_settings.save_candidate = self.audio_settings.sound.clone();
                self.audio_settings.save_after = Some(Instant::now() + Duration::from_millis(500));
            }
        }
        if !has_audio {
            ui.small("Enhancements will apply when audio starts.");
        }
        ui.separator();
        ui.strong("Audio / video sync");
        ui.add_enabled_ui(ready, |ui| {
            let mut milliseconds = self.audio_settings.requested_delay.unwrap_or(delay) * 1000.0;
            if ui
                .add(
                    egui::Slider::new(&mut milliseconds, -5000.0..=5000.0)
                        .text("Audio delay")
                        .suffix(" ms")
                        .step_by(10.0),
                )
                .changed()
            {
                self.audio_settings.requested_delay = Some(milliseconds / 1000.0);
                self.audio_request(
                    &[
                        "set",
                        "audio-delay",
                        &format!("{:.3}", milliseconds / 1000.0),
                    ],
                    Pending::Delay,
                );
            }
            ui.small("Positive values play audio later; negative values play it earlier.");
            if ui.button("Reset sync").clicked() {
                self.audio_settings.requested_delay = Some(0.0);
                self.audio_request(&["set", "audio-delay", "0"], Pending::Delay);
            }
        });
        ui.separator();
        egui::CollapsingHeader::new("Mix multiple audio tracks").default_open(true).show(ui, |ui| {
            ui.small("Play movie audio and commentary together. Select two to four tracks, set their relative levels, then apply. Mixing resets for each file.");
            ui.add_enabled_ui(ready && !self.audio_settings.has_mix_pending(), |ui| {
                for track in &tracks {
                    let mut selected = self.audio_settings.mix.iter().any(|t| t.id == track.id);
                    if ui.checkbox(&mut selected, track.label()).changed() {
                        if selected && self.audio_settings.mix.len() < 4 { self.audio_settings.mix.push(MixInput { id: track.id, level: 1.0 }); }
                        if !selected { self.audio_settings.mix.retain(|t| t.id != track.id); }
                    }
                    if let Some(input) = self.audio_settings.mix.iter_mut().find(|t| t.id == track.id) {
                        ui.add(egui::Slider::new(&mut input.level, 0.0..=1.0).text("Relative level").custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                    }
                }
                if tracks.len() < 2 { ui.small("This media needs at least two audio tracks to mix."); }
                let valid = mix_graph(&self.audio_settings.mix).is_ok();
                if ui.add_enabled(valid, egui::Button::new("Apply track mix")).clicked() {
                    if self.audio_settings.applied_mix.is_empty() { self.audio_settings.single_track = if aid.is_empty() { "auto".into() } else { aid.clone() }; }
                    let inputs = self.audio_settings.mix.clone();
                    let graph = mix_graph(&inputs).unwrap();
                    self.audio_request(&["set", "lavfi-complex", &graph], Pending::Mix(inputs));
                }
                if ui.add_enabled(!self.audio_settings.applied_mix.is_empty(), egui::Button::new("Return to single track")).clicked() {
                    self.audio_request(&["set", "lavfi-complex", ""], Pending::Mix(vec![]));
                    let track = self.audio_settings.single_track.clone();
                    self.audio_request(&["set", "aid", &track], Pending::Other);
                    self.audio_settings.mix.clear();
                }
                if !self.audio_settings.applied_mix.is_empty() { ui.label(format!("Mixing {} tracks · levels normalized", self.audio_settings.applied_mix.len())); }
            });
        });
        if !self.audio_settings.pending.is_empty() {
            ui.small("Applying audio changes…");
        }
        if let Some(error) = &self.audio_settings.engine_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(error) = &self.audio_settings.save_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Save audio settings again").clicked() {
                self.audio_settings.save_after = Some(Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_preferences_round_trip_and_reject_invalid_values() {
        let directory =
            std::env::temp_dir().join(format!("nvplay-audio-prefs-{}", std::process::id()));
        let path = directory.join("audio.json");
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(Sound::read(&path).unwrap(), Sound::default());
        let sound = Sound {
            enabled: true,
            gains: PRESETS[1].1,
            preamp: -2.0,
            balance: -0.4,
            mono: true,
            night: true,
            leveling: true,
            ..Sound::default()
        };
        sound.write(&path).unwrap();
        assert_eq!(Sound::read(&path).unwrap(), sound);
        let mut invalid = sound.clone();
        invalid.gains[0] = 1000.0;
        assert!(invalid.write(&path).is_err());
        assert_eq!(
            Sound::read(&path).unwrap(),
            sound,
            "invalid edits must not overwrite settings"
        );
        std::fs::write(&path, br#"{"balance":2}"#).unwrap();
        assert!(Sound::read(&path).is_err());
        std::fs::write(&path, b"broken").unwrap();
        assert!(Sound::read(&path).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn mixer_rejects_invalid_routes_and_failed_sound_is_not_saved() {
        assert!(mix_graph(&[MixInput { id: 1, level: 1.0 }]).is_err());
        assert!(
            mix_graph(&[
                MixInput { id: 1, level: 0.0 },
                MixInput { id: 2, level: 0.0 }
            ])
            .is_err()
        );
        assert!(
            mix_graph(&[
                MixInput { id: 1, level: 1.0 },
                MixInput { id: 1, level: 0.5 }
            ])
            .is_err()
        );
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        let changed = Sound {
            enabled: true,
            gains: PRESETS[1].1,
            ..Sound::default()
        };
        let id = crate::playback::AUDIO_SETTINGS_TAG;
        app.audio_settings.sound = changed.clone();
        app.audio_settings
            .pending
            .insert(id, Pending::Sound(changed));
        app.audio_settings_applied(id, Err("unsupported filter".into()));
        assert_eq!(app.audio_settings.sound, Sound::default());
        assert_eq!(app.audio_settings.save_candidate, Sound::default());
        assert!(app.audio_settings.engine_error.is_some());
    }

    fn wait_for(app: &mut PlayerApp, condition: impl Fn(&PlayerApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            for event in app.player.as_mut().unwrap().poll() {
                app.handle_playback_event(event);
            }
            let ctx = app.ctx.clone();
            app.poll_audio_settings(&ctx);
            assert!(app.error.is_none(), "{:?}", app.error);
            if condition(app) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "audio did not settle: {:?}",
                app.audio_settings.engine_error
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    #[ignore = "requires libmpv and NVPLAY_TEST_MEDIA with at least two audio tracks; null output, no sound"]
    fn real_audio_equalizer_mixing_external_sources_and_recovery() {
        let ctx = egui::Context::default();
        let player = Player::create("null", Some("null"), Some(&ctx)).unwrap();
        let source = std::env::var_os("NVPLAY_TEST_MEDIA").expect("NVPLAY_TEST_MEDIA");
        let mut app = PlayerApp::from_parts(&ctx, Some(player), None, None, vec![source]);
        wait_for(&mut app, |a| {
            !a.loading
                && a.player
                    .as_ref()
                    .unwrap()
                    .state
                    .tracks
                    .iter()
                    .filter(|t| t.kind == "audio")
                    .count()
                    >= 2
        });
        for (_, gains) in PRESETS {
            let sound = Sound {
                enabled: true,
                gains,
                balance: 0.3,
                night: true,
                leveling: true,
                ..Sound::default()
            };
            app.audio_settings.sound = sound.clone();
            app.audio_settings.apply_after = Some(Instant::now());
            wait_for(&mut app, |a| {
                !a.audio_settings.has_sound_pending() && a.audio_settings.applied == sound
            });
            assert!(
                app.audio_settings.engine_error.is_none(),
                "{:?}",
                app.audio_settings.engine_error
            );
            assert!(
                app.player
                    .as_ref()
                    .unwrap()
                    .test_property("af")
                    .unwrap()
                    .contains("nvplay-audio")
            );
        }
        app.audio_settings.sound.mono = true;
        app.audio_settings.apply_after = Some(Instant::now());
        wait_for(&mut app, |a| {
            !a.audio_settings.has_sound_pending() && a.audio_settings.applied.mono
        });
        wait_for(&mut app, |a| {
            a.player
                .as_ref()
                .unwrap()
                .test_property("audio-out-params/channel-count")
                .as_deref()
                == Some("1")
        });
        app.audio_settings.requested_delay = Some(0.25);
        app.audio_request(&["set", "audio-delay", "0.25"], Pending::Delay);
        wait_for(&mut app, |a| {
            (a.player.as_ref().unwrap().state.audio_delay - 0.25).abs() < 0.001
        });
        if let Some(path) = std::env::var_os("NVPLAY_TEST_AUDIO_FILE") {
            let before = app.player.as_ref().unwrap().state.tracks.len();
            app.add_external_audio(path.into());
            wait_for(&mut app, |a| {
                a.player.as_ref().unwrap().state.tracks.len() > before
            });
        }
        let tracks: Vec<_> = app
            .player
            .as_ref()
            .unwrap()
            .state
            .tracks
            .iter()
            .filter(|t| t.kind == "audio")
            .take(2)
            .map(|t| MixInput {
                id: t.id,
                level: if t.id == 1 { 1.0 } else { 0.3 },
            })
            .collect();
        app.audio_settings.mix = tracks.clone();
        app.audio_request(
            &["set", "lavfi-complex", &mix_graph(&tracks).unwrap()],
            Pending::Mix(tracks),
        );
        wait_for(&mut app, |a| a.audio_settings.applied_mix.len() == 2);
        assert!(app.audio_settings.engine_error.is_none());
        assert!(
            app.player
                .as_ref()
                .unwrap()
                .test_property("lavfi-complex")
                .unwrap()
                .contains("amix")
        );
        app.command(&["set", "aid", "1"]);
        wait_for(&mut app, |a| {
            a.player.as_ref().unwrap().state.audio_track == "1"
                && a.player
                    .as_ref()
                    .unwrap()
                    .test_property("lavfi-complex")
                    .is_some_and(|g| g.is_empty())
        });
        assert!(app.audio_settings.applied_mix.is_empty());
        assert!(
            app.player
                .as_ref()
                .unwrap()
                .test_property("lavfi-complex")
                .unwrap()
                .is_empty()
        );
        let valid = app.audio_settings.applied.clone();
        app.audio_request(
            &[
                "af",
                "add",
                "@nvplay-audio:lavfi=[filter_that_does_not_exist]",
            ],
            Pending::Sound(Sound::default()),
        );
        wait_for(&mut app, |a| {
            a.audio_settings.engine_error.is_some() && a.audio_settings.pending.is_empty()
        });
        assert_eq!(app.audio_settings.applied, valid);
        assert_eq!(app.audio_settings.sound, valid);
        app.audio_settings.engine_error = None;
        app.play(0);
        wait_for(&mut app, |a| {
            !a.loading
                && a.audio_settings.pending.is_empty()
                && a.audio_settings.apply_after.is_none()
        });
        assert!(app.audio_settings.applied_mix.is_empty());
        assert!(
            app.player
                .as_ref()
                .unwrap()
                .test_property("lavfi-complex")
                .unwrap()
                .is_empty()
        );
        assert!(app.audio_settings.engine_error.is_none());
        app.stop();
    }
}
