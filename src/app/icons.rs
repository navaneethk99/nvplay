use eframe::egui::{self, Pos2, Stroke, Vec2, vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    SkipBackward,
    SkipForward,
    Previous,
    Next,
    Fullscreen,
    Playlist,
    Repeat,
    Volume,
    Muted,
}

pub fn button(ui: &mut egui::Ui, icon: Icon, help: &str, selected: bool) -> egui::Response {
    let response = ui.add_sized([28.0, 28.0], egui::Button::new("").selected(selected));
    let rect = response.rect;
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), help));
    if ui.is_rect_visible(rect) {
        let color = super::ACCENT;
        let stroke = Stroke::new(1.4_f32, color);
        let center = rect.center();
        let p = |x, y| center + vec2(x, y);
        let line = |a: Pos2, b: Pos2| {
            ui.painter().line_segment([a, b], stroke);
        };
        let triangle = |x: f32, direction: f32| {
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    p(x - 3.0 * direction, -5.0),
                    p(x + 4.0 * direction, 0.0),
                    p(x - 3.0 * direction, 5.0),
                ],
                color,
                Stroke::NONE,
            ));
        };
        match icon {
            Icon::Play => triangle(0.0, 1.0),
            Icon::Pause => {
                for x in [-4.0, 1.0] {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(p(x, -5.0), vec2(3.0, 10.0)),
                        0.0,
                        color,
                    );
                }
            }
            Icon::Stop => {
                ui.painter().rect_filled(
                    egui::Rect::from_center_size(center, Vec2::splat(9.0)),
                    0.0,
                    color,
                );
            }
            Icon::SkipBackward | Icon::SkipForward => {
                let direction = if matches!(icon, Icon::SkipBackward) {
                    -1.0
                } else {
                    1.0
                };
                triangle(-4.0 * direction, direction);
                triangle(3.0 * direction, direction);
            }
            Icon::Previous | Icon::Next => {
                let direction = if matches!(icon, Icon::Previous) {
                    -1.0
                } else {
                    1.0
                };
                triangle(-direction, direction);
                line(p(5.0 * direction, -5.0), p(5.0 * direction, 5.0));
            }
            Icon::Fullscreen => {
                for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    line(p(x * 6.0, y * 2.0), p(x * 6.0, y * 6.0));
                    line(p(x * 6.0, y * 6.0), p(x * 2.0, y * 6.0));
                }
            }
            Icon::Playlist => {
                for y in [-4.0, 0.0, 4.0] {
                    line(p(-2.0, y), p(6.0, y));
                    ui.painter().circle_filled(p(-5.0, y), 0.9, color);
                }
            }
            Icon::Repeat => {
                for direction in [-1.0, 1.0] {
                    line(
                        p(-5.0 * direction, -4.0 * direction),
                        p(5.0 * direction, -4.0 * direction),
                    );
                    line(
                        p(5.0 * direction, -4.0 * direction),
                        p(5.0 * direction, 1.0 * direction),
                    );
                    line(
                        p(-5.0 * direction, -4.0 * direction),
                        p(-2.0 * direction, -7.0 * direction),
                    );
                }
            }
            Icon::Volume | Icon::Muted => {
                ui.painter().add(egui::Shape::convex_polygon(
                    vec![
                        p(-6.0, -2.0),
                        p(-3.0, -2.0),
                        p(1.0, -5.0),
                        p(1.0, 5.0),
                        p(-3.0, 2.0),
                        p(-6.0, 2.0),
                    ],
                    color,
                    Stroke::NONE,
                ));
                if matches!(icon, Icon::Muted) {
                    line(p(4.0, -3.0), p(8.0, 3.0));
                    line(p(8.0, -3.0), p(4.0, 3.0));
                } else {
                    line(p(4.0, -4.0), p(7.0, 0.0));
                    line(p(7.0, 0.0), p(4.0, 4.0));
                }
            }
        }
    }
    response.on_hover_text(help)
}

/// Transparent brand mark used on the empty or stopped video surface.
pub fn load_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let image = image::load_from_memory(include_bytes!("assets/nvplay-mark.png"))
        .expect("bundled nvplay logo must be a valid PNG")
        .to_rgba8();
    ctx.load_texture(
        "nvplay-brand-mark",
        egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    )
}
pub fn logo(painter: &egui::Painter, bounds: egui::Rect, texture: &egui::TextureHandle) {
    let image_size = texture.size_vec2();
    let available = (bounds.size() - Vec2::splat(48.0)).max(Vec2::ZERO);
    let scale = (112.0 / image_size.x)
        .min(available.x / image_size.x)
        .min(available.y / image_size.y);
    let rect = egui::Rect::from_center_size(bounds.center(), image_size * scale);
    painter.image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}
