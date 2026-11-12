#![forbid(unsafe_code)]

mod app;
mod overlay;
mod pdf;
mod render;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Typewriter PDF",
        opts,
        Box::new(|cc| Ok(Box::new(app::TypewriterApp::new(cc)))),
    )
}
