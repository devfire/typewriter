#![forbid(unsafe_code)]

mod app;
mod overlay;
mod pdf;
mod render;
mod signature;
#[cfg(test)]
mod test_util;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Typewriter PDF",
        opts,
        Box::new(|cc| Ok(Box::new(app::TypewriterApp::new(cc)))),
    )
}
