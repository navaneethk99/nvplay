//! App-wide appearance and the persistent theme picker.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Theme {
    #[default]
    Original,
    Light,
    Dark,
    Monochrome,
    LiquidGlass,
    Cyber,
    Rain,
    Vercel,
    Aurora,
    Sunset,
    Forest,
    RosePine,
    Ocean,
    Sand,
    Terminal,
    Lavender,
    VideoAurora,
}

impl Theme {
    pub const ALL: [Self; 17] = [
        Self::Original,
        Self::Light,
        Self::Dark,
        Self::Monochrome,
        Self::LiquidGlass,
        Self::Cyber,
        Self::Rain,
        Self::Vercel,
        Self::Aurora,
        Self::Sunset,
        Self::Forest,
        Self::RosePine,
        Self::Ocean,
        Self::Sand,
        Self::Terminal,
        Self::Lavender,
        Self::VideoAurora,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Monochrome => "Monochrome",
            Self::LiquidGlass => "Liquid Glass",
            Self::Cyber => "Cyber",
            Self::Rain => "Rain",
            Self::Vercel => "Vercel",
            Self::Aurora => "Aurora",
            Self::Sunset => "Sunset",
            Self::Forest => "Forest",
            Self::RosePine => "Rosé Pine",
            Self::Ocean => "Ocean",
            Self::Sand => "Sand",
            Self::Terminal => "Terminal",
            Self::Lavender => "Lavender",
            Self::VideoAurora => "Video Aurora",
        }
    }

    pub fn is_grayscale(self) -> bool {
        matches!(self, Self::Monochrome | Self::Vercel)
    }

    fn description(self) -> &'static str {
        match self {
            Self::Original => "The classic charcoal player with warm orange controls.",
            Self::Light => "Bright surfaces, dark text, and blue accents.",
            Self::Dark => "Deep slate surfaces with cool blue accents.",
            Self::Monochrome => "Black and gray surfaces with grayscale controls.",
            Self::Cyber => {
                "Neon cyan controls against midnight violet, inspired by a city after dark."
            }
            Self::Rain => "Rainy blue-gray surfaces with soft mist-blue accents.",
            Self::Vercel => "A crisp black canvas, white controls, and minimal borders.",
            Self::Aurora => "Polar-night blues with luminous mint-green accents.",
            Self::Sunset => "Warm plum surfaces and glowing peach controls at golden hour.",
            Self::Forest => "Deep evergreen surfaces with fresh sage accents.",
            Self::RosePine => "Muted pine-night surfaces with dusty rose highlights.",
            Self::Ocean => "Deep ocean blues with bright turquoise controls.",
            Self::Sand => "Warm ivory surfaces, espresso text, and terracotta accents.",
            Self::Terminal => "Phosphor-green controls on a near-black terminal canvas.",
            Self::Lavender => "Soft lilac surfaces with rich violet accents.",
            Self::VideoAurora => {
                "A flowing aurora that follows the colors of your video. Colors blend gently as the scene changes."
            }
            Self::LiquidGlass => {
                "Translucent pearl surfaces, soft highlights, and rounded controls inspired by Apple platforms."
            }
        }
    }

    pub fn load() -> Result<Self, String> {
        let path = crate::settings::directory()?.join("appearance.json");
        Self::read(&path)
    }

    fn read(path: &std::path::Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Could not read the saved theme. Using Original.".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Original),
            Err(_) => Err("Could not read the saved theme. Using Original.".into()),
        }
    }

    fn save(self) -> Result<(), String> {
        let path = crate::settings::directory()?.join("appearance.json");
        self.write(&path)
    }

    fn write(self, path: &std::path::Path) -> Result<(), String> {
        crate::settings::write_private(path, &serde_json::to_vec(&self).map_err(|e| e.to_string())?)
            .map_err(|_| {
                "Your theme is applied, but could not be saved. Try again to keep it after restart."
                    .into()
            })
    }

    pub fn visuals(self) -> egui::Visuals {
        if self == Self::Original {
            return chrome_visuals();
        }
        let light = matches!(
            self,
            Self::Light | Self::LiquidGlass | Self::Sand | Self::Lavender
        );
        let mut v = if light {
            egui::Visuals::light()
        } else {
            egui::Visuals::dark()
        };
        let (panel, window, accent) = match self {
            Self::Light => (
                Color32::from_rgb(240, 243, 248),
                Color32::from_rgb(250, 251, 253),
                Color32::from_rgb(25, 83, 173),
            ),
            Self::Dark => (
                Color32::from_rgb(24, 28, 36),
                Color32::from_rgb(30, 35, 44),
                Color32::from_rgb(134, 181, 255),
            ),
            Self::Monochrome => (
                Color32::from_gray(20),
                Color32::from_gray(29),
                Color32::from_gray(220),
            ),
            Self::LiquidGlass => (
                Color32::from_rgba_unmultiplied(224, 233, 245, 230),
                Color32::from_rgba_unmultiplied(240, 246, 255, 245),
                Color32::from_rgb(32, 79, 151),
            ),
            Self::Cyber => (
                Color32::from_rgb(17, 12, 32),
                Color32::from_rgb(25, 18, 43),
                Color32::from_rgb(63, 232, 242),
            ),
            Self::Rain => (
                Color32::from_rgb(27, 37, 47),
                Color32::from_rgb(34, 46, 58),
                Color32::from_rgb(160, 199, 218),
            ),
            Self::Vercel => (
                Color32::from_rgb(0, 0, 0),
                Color32::from_rgb(10, 10, 10),
                Color32::from_rgb(245, 245, 245),
            ),
            Self::Aurora => (
                Color32::from_rgb(14, 29, 40),
                Color32::from_rgb(22, 40, 50),
                Color32::from_rgb(111, 232, 191),
            ),
            Self::Sunset => (
                Color32::from_rgb(43, 25, 39),
                Color32::from_rgb(57, 34, 48),
                Color32::from_rgb(255, 183, 133),
            ),
            Self::Forest => (
                Color32::from_rgb(16, 33, 27),
                Color32::from_rgb(24, 45, 35),
                Color32::from_rgb(162, 204, 157),
            ),
            Self::RosePine => (
                Color32::from_rgb(31, 29, 46),
                Color32::from_rgb(38, 35, 58),
                Color32::from_rgb(235, 160, 172),
            ),
            Self::Ocean => (
                Color32::from_rgb(9, 30, 48),
                Color32::from_rgb(15, 42, 62),
                Color32::from_rgb(81, 218, 217),
            ),
            Self::Sand => (
                Color32::from_rgb(242, 232, 212),
                Color32::from_rgb(251, 244, 231),
                Color32::from_rgb(143, 67, 42),
            ),
            Self::Terminal => (
                Color32::from_rgb(8, 17, 11),
                Color32::from_rgb(13, 25, 17),
                Color32::from_rgb(115, 230, 133),
            ),
            Self::Lavender => (
                Color32::from_rgb(237, 231, 248),
                Color32::from_rgb(248, 244, 255),
                Color32::from_rgb(109, 64, 163),
            ),
            Self::VideoAurora => (
                Color32::from_rgb(12, 19, 30),
                Color32::from_rgb(17, 25, 37),
                Color32::from_rgb(160, 220, 215),
            ),
            Self::Original => unreachable!(),
        };
        v.panel_fill = panel;
        v.window_fill = window;
        v.hyperlink_color = accent;
        v.selection.bg_fill = if light {
            Color32::from_rgb(205, 222, 249)
        } else {
            Color32::from_rgb(43, 62, 90)
        };
        // Give each additional palette matching inputs, buttons, and borders too.
        if !matches!(
            self,
            Self::Light | Self::Dark | Self::Monochrome | Self::LiquidGlass
        ) {
            let mix = |a: Color32, b: Color32, amount: u16| {
                let channel = |a: u8, b: u8| {
                    ((u16::from(a) * (100 - amount) + u16::from(b) * amount) / 100) as u8
                };
                Color32::from_rgb(
                    channel(a.r(), b.r()),
                    channel(a.g(), b.g()),
                    channel(a.b(), b.b()),
                )
            };
            v.extreme_bg_color = mix(
                panel,
                if light {
                    Color32::WHITE
                } else {
                    Color32::BLACK
                },
                30,
            );
            v.faint_bg_color = window;
            v.code_bg_color = window;
            v.window_stroke.color = mix(window, accent, 25);
            v.widgets.noninteractive.bg_fill = window;
            v.widgets.noninteractive.weak_bg_fill = window;
            v.widgets.noninteractive.bg_stroke.color = mix(window, accent, 25);
            v.widgets.inactive.bg_fill = mix(window, accent, 12);
            v.widgets.inactive.weak_bg_fill = window;
            v.widgets.inactive.bg_stroke.color = mix(window, accent, 25);
            v.widgets.hovered.bg_fill = mix(window, accent, 25);
            v.widgets.hovered.weak_bg_fill = mix(window, accent, 12);
            v.widgets.hovered.bg_stroke.color = accent;
            v.widgets.active.bg_fill = mix(window, accent, 35);
            v.widgets.active.weak_bg_fill = mix(window, accent, 20);
            v.widgets.active.bg_stroke.color = accent;
            v.widgets.open = v.widgets.active;
            v.selection.bg_fill = mix(panel, accent, if light { 20 } else { 25 });
        }
        if self.is_grayscale() {
            v.selection.bg_fill = Color32::from_gray(65);
            v.warn_fg_color = Color32::from_gray(210);
            v.error_fg_color = Color32::from_gray(230);
        }
        v.selection.stroke.color = accent;
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius =
                egui::CornerRadius::same(if self == Self::LiquidGlass { 10 } else { 4 });
        }
        if self == Self::LiquidGlass {
            v.window_corner_radius = egui::CornerRadius::same(18);
            v.window_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
            v.widgets.inactive.weak_bg_fill = Color32::from_white_alpha(90);
            v.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(175);
            v.widgets.active.weak_bg_fill = Color32::from_rgb(196, 213, 237);
            v.widgets.inactive.bg_stroke =
                egui::Stroke::new(1.0_f32, Color32::from_white_alpha(210));
        }
        v
    }

    pub fn apply(self, ctx: &egui::Context) {
        theme(ctx);
        ctx.set_visuals(self.visuals());
        ctx.request_repaint();
    }
}

impl PlayerApp {
    pub(super) fn chrome_frame(&self, ctx: &egui::Context) -> egui::Frame {
        let frame = egui::Frame::new().fill(ctx.style().visuals.panel_fill);
        if self.appearance == Theme::LiquidGlass {
            frame
                .corner_radius(10)
                .stroke(egui::Stroke::new(1.0_f32, Color32::from_white_alpha(180)))
        } else {
            frame
        }
    }

    pub(super) fn open_settings(&mut self) {
        self.show_settings = true;
    }

    pub(super) fn appearance_settings_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut selected = self.appearance;
        let mut retry = false;

        ui.heading("Appearance");
        ui.label("Choose a theme. Changes apply immediately.");
        ui.add_space(8.0);
        egui::ComboBox::from_label("Theme")
            .selected_text(selected.label())
            .width(220.0)
            .height(240.0)
            .show_ui(ui, |ui| {
                for option in Theme::ALL {
                    ui.selectable_value(&mut selected, option, option.label())
                        .on_hover_text(option.description());
                }
            });
        ui.horizontal(|ui| {
            let palette = if selected == Theme::VideoAurora && selected == self.appearance {
                (*ctx.style()).visuals.clone()
            } else {
                selected.visuals()
            };
            for color in [
                palette.panel_fill,
                palette.widgets.noninteractive.fg_stroke.color,
                palette.selection.stroke.color,
            ] {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 8.0, color);
                ui.painter().rect_stroke(
                    rect,
                    8.0,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                    egui::StrokeKind::Inside,
                );
            }
        });
        ui.add_space(8.0);
        ui.label(selected.description());
        if selected == Theme::LiquidGlass {
            ui.small("Glass styling uses translucency; native background blur is not available in this renderer.");
        }
        if let Some(error) = &self.appearance_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            retry = ui.button("Save theme again").clicked();
        }

        if selected != self.appearance || retry {
            self.appearance = selected;
            selected.apply(&ctx);
            if selected == Theme::VideoAurora {
                self.video_aurora = video_aurora::VideoAurora::default();
            }
            self.brand_logo = icons::load_logo(&ctx, selected.is_grayscale());
            self.appearance_error = selected.save().err();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_preferences_survive_restart_and_report_invalid_data() {
        let directory =
            std::env::temp_dir().join(format!("nvplay-theme-test-{}", std::process::id()));
        let path = directory.join("appearance.json");
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(Theme::read(&path).unwrap(), Theme::Original);
        for theme in Theme::ALL {
            theme.write(&path).unwrap();
            assert_eq!(Theme::read(&path).unwrap(), theme);
        }
        std::fs::write(&path, b"broken json").unwrap();
        assert!(Theme::read(&path).is_err());
        assert!(Theme::Dark.write(&path.join("invalid")).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn themes_round_trip_and_have_distinct_palettes() {
        for (index, theme) in Theme::ALL.into_iter().enumerate() {
            let bytes = serde_json::to_vec(&theme).unwrap();
            assert_eq!(serde_json::from_slice::<Theme>(&bytes).unwrap(), theme);
            for other in &Theme::ALL[..index] {
                assert_ne!(theme.visuals().panel_fill, other.visuals().panel_fill);
            }
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            assert_eq!(ctx.style().visuals.panel_fill, theme.visuals().panel_fill);
        }
        assert!(serde_json::from_str::<Theme>("\"unknown\"").is_err());
    }
}
