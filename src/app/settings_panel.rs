//! Settings navigation shared by appearance, audio, and key bindings.
use super::*;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum SettingsTab {
    #[default]
    Appearance,
    Audio,
    KeyBindings,
}

impl PlayerApp {
    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .min_width(300.0)
            .max_width((ctx.content_rect().width() - 32.0).max(300.0))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut self.settings_tab,
                        SettingsTab::Appearance,
                        "Appearance",
                    );
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::Audio, "Audio");
                    ui.selectable_value(
                        &mut self.settings_tab,
                        SettingsTab::KeyBindings,
                        "Key Bindings",
                    );
                });
                if self.settings_tab != SettingsTab::KeyBindings {
                    self.key_bindings.capturing = None;
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt(("settings_body", self.settings_tab))
                    .max_height((ctx.content_rect().height() - 120.0).max(80.0))
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        match self.settings_tab {
                            SettingsTab::Appearance => self.appearance_settings_ui(ui),
                            SettingsTab::Audio => self.audio_settings_ui(ui),
                            SettingsTab::KeyBindings => self.key_bindings_settings_ui(ui),
                        }
                    });
            });
        self.show_settings = open;
        if !open {
            self.key_bindings.capturing = None;
        }
    }
}
