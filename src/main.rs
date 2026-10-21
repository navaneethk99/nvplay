mod app;
mod open_documents;
mod playback;
#[cfg(target_os = "macos")]
mod source_version;
mod streaming;

fn main() -> eframe::Result {
    let media = std::env::args_os().skip(1).collect();
    let open_documents = open_documents::OpenDocuments::new();
    eframe::run_native(
        "nvplay",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            vsync: true,
            viewport: eframe::egui::ViewportBuilder::default()
                .with_icon(
                    eframe::icon_data::from_png_bytes(include_bytes!("app/assets/nvplay.png"))
                        .expect("bundled nvplay icon must be a valid PNG"),
                )
                .with_inner_size([640.0, 400.0])
                .with_min_inner_size([440.0, 260.0]),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(app::PlayerApp::new(cc, media, open_documents)))),
    )
}
