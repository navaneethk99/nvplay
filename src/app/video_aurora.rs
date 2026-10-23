//! Smoothed scene colors and a lightweight animated mesh behind player chrome.
use super::*;
use crate::playback::VideoPalette;

const FALLBACK: VideoPalette = [
    Color32::from_rgb(65, 155, 155),
    Color32::from_rgb(113, 85, 160),
    Color32::from_rgb(63, 101, 160),
];

pub(super) struct VideoAurora {
    colors: [[f32; 3]; 3],
    updated: Instant,
    phase: f32,
    moving: bool,
    settling: bool,
}

impl Default for VideoAurora {
    fn default() -> Self {
        Self {
            colors: FALLBACK.map(channels),
            updated: Instant::now(),
            phase: 0.0,
            moving: false,
            settling: false,
        }
    }
}

fn channels(color: Color32) -> [f32; 3] {
    [
        f32::from(color.r()),
        f32::from(color.g()),
        f32::from(color.b()),
    ]
}
fn color(rgb: [f32; 3]) -> Color32 {
    Color32::from_rgb(
        rgb[0].round() as u8,
        rgb[1].round() as u8,
        rgb[2].round() as u8,
    )
}
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let a = channels(a);
    let b = channels(b);
    color(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t))
}

impl VideoAurora {
    fn step(&mut self, target: VideoPalette, dt: f32) {
        let amount = 1.0 - (-dt / 0.8).exp();
        self.settling = false;
        for (current, target) in self.colors.iter_mut().zip(target.map(channels)) {
            for i in 0..3 {
                current[i] += (target[i] - current[i]) * amount;
                self.settling |= (target[i] - current[i]).abs() > 0.5;
            }
        }
        if self.moving {
            self.phase = (self.phase + dt * 0.3) % std::f32::consts::TAU;
        }
    }

    fn palette(&self) -> VideoPalette {
        self.colors.map(color)
    }

    fn visuals(&self) -> egui::Visuals {
        let colors = self.palette();
        let surface = mix(Color32::from_rgb(10, 14, 22), colors[0], 0.12);
        let window = mix(surface, colors[1], 0.06);
        // A controlled dark surface and a lifted accent keep scene changes from
        // making buttons or text illegible, even for black or white footage.
        let accent = mix(colors[0], Color32::WHITE, 0.65);
        let mut v = appearance::Theme::VideoAurora.visuals();
        v.panel_fill = surface;
        v.window_fill = window;
        v.extreme_bg_color = mix(surface, Color32::BLACK, 0.3);
        v.faint_bg_color = window;
        v.code_bg_color = window;
        v.hyperlink_color = accent;
        v.selection.bg_fill = mix(surface, accent, 0.22);
        v.selection.stroke.color = accent;
        v.window_stroke.color = mix(window, accent, 0.2);
        v.widgets.noninteractive.bg_stroke.color = v.window_stroke.color;
        v.widgets.inactive.bg_fill = mix(window, accent, 0.12);
        v.widgets.inactive.weak_bg_fill = window;
        v.widgets.inactive.bg_stroke.color = v.window_stroke.color;
        v.widgets.hovered.bg_fill = mix(window, accent, 0.25);
        v.widgets.hovered.weak_bg_fill = mix(window, accent, 0.12);
        v.widgets.hovered.bg_stroke.color = accent;
        v.widgets.active.bg_fill = mix(window, accent, 0.35);
        v.widgets.active.weak_bg_fill = mix(window, accent, 0.2);
        v.widgets.active.bg_stroke.color = accent;
        v.widgets.open = v.widgets.active;
        v
    }

    fn paint(&self, ui: &egui::Ui) {
        let rect = ui.max_rect();
        if !rect.is_positive() || !rect.is_finite() {
            return;
        }
        let mut mesh = egui::Mesh::default();
        let colors = self.palette();
        let base = ui.visuals().panel_fill;
        const COLS: u32 = 48;
        const ROWS: u32 = 6;
        for y in 0..=ROWS {
            let fy = y as f32 / ROWS as f32;
            for x in 0..=COLS {
                let fx = x as f32 / COLS as f32;
                let ribbon = 0.5 + 0.26 * (fx * std::f32::consts::TAU + self.phase).sin();
                let strength = 0.12 + 0.22 * (-((fy - ribbon) / 0.35).powi(2)).exp();
                let hue = fx * 2.0;
                let index = (hue.floor() as usize).min(1);
                let tint = mix(colors[index], colors[index + 1], hue - index as f32);
                // Cap the glow, including white scenes, below the brightness
                // of secondary text and control icons without changing its hue.
                let glow = mix(base, tint, strength);
                let scale =
                    (72.0 / f32::from(glow.r().max(glow.g()).max(glow.b()).max(1))).min(1.0);
                let glow = color(channels(glow).map(|channel| channel * scale));
                mesh.colored_vertex(
                    egui::pos2(
                        rect.left() + fx * rect.width(),
                        rect.top() + fy * rect.height(),
                    ),
                    glow,
                );
                if x < COLS && y < ROWS {
                    let i = y * (COLS + 1) + x;
                    mesh.add_triangle(i, i + 1, i + COLS + 1);
                    mesh.add_triangle(i + 1, i + COLS + 2, i + COLS + 1);
                }
            }
        }
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .add(mesh);
        if self.moving || self.settling {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }
}

impl PlayerApp {
    pub(super) fn update_video_aurora(&mut self, ctx: &egui::Context) {
        let selected = self.appearance == appearance::Theme::VideoAurora;
        let has_video = self.current.is_some()
            && !self.loading
            && self.player.as_ref().is_some_and(|p| {
                !p.state.idle
                    && p.state.video_width > 0
                    && p.state
                        .tracks
                        .iter()
                        .any(|t| t.kind == "video" && t.selected)
            });
        let palette = self.renderer.lock().ok().and_then(|mut renderer| {
            renderer
                .as_mut()
                .and_then(|r| r.ambient_palette(selected && has_video, self.playback_generation))
        });
        let now = Instant::now();
        let dt = now
            .duration_since(self.video_aurora.updated)
            .as_secs_f32()
            .min(0.1);
        self.video_aurora.updated = now;
        if !selected {
            return;
        }
        self.video_aurora.moving = has_video
            && self
                .player
                .as_ref()
                .is_some_and(|p| !p.state.paused && !p.state.buffering);
        let previous = self.video_aurora.palette();
        self.video_aurora.step(palette.unwrap_or(FALLBACK), dt);
        if previous != self.video_aurora.palette()
            || ctx.style().visuals.panel_fill == appearance::Theme::VideoAurora.visuals().panel_fill
        {
            ctx.set_visuals(self.video_aurora.visuals());
        }
    }

    pub(super) fn paint_video_aurora(&self, ui: &egui::Ui) {
        if self.appearance == appearance::Theme::VideoAurora {
            self.video_aurora.paint(ui);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_changes_blend_smoothly_and_pause_freezes_the_ribbons() {
        let mut aurora = VideoAurora::default();
        let before = aurora.colors;
        let target = [Color32::RED; 3];
        aurora.step(target, 0.1);
        assert!(aurora.colors[0][0] > before[0][0] && aurora.colors[0][0] < 255.0);
        assert!(aurora.settling);
        assert_eq!(aurora.phase, 0.0);
        aurora.moving = true;
        for _ in 0..100 {
            aurora.step(target, 0.1);
        }
        assert_eq!(aurora.palette(), target);
        assert!(!aurora.settling);
        assert!(aurora.phase > 0.0);
    }

    #[test]
    fn extreme_scene_colors_keep_dark_surfaces_and_visible_accents() {
        for scene in [Color32::BLACK, Color32::WHITE, Color32::RED, Color32::BLUE] {
            let mut aurora = VideoAurora::default();
            aurora.step([scene; 3], 100.0);
            let v = aurora.visuals();
            assert!(v.panel_fill.r().max(v.panel_fill.g()).max(v.panel_fill.b()) < 55);
            let accent = v.selection.stroke.color;
            assert!(accent.r().min(accent.g()).min(accent.b()) >= 165);
            let ctx = egui::Context::default();
            ctx.set_visuals(v);
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| aurora.paint(ui));
            });
            let mesh = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .expect("aurora mesh");
            for vertex in &mesh.vertices {
                let c = vertex.color;
                assert!(
                    c.r().max(c.g()).max(c.b()) <= 72,
                    "bright scene washed out the chrome"
                );
            }
        }
    }
}
