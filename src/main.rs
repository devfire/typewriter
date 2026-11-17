#![forbid(unsafe_code)]

mod app;
mod cli;
mod geometry;
mod overlay;
mod pdf;
mod render;
mod signature;
#[cfg(test)]
mod test_util;
mod text_font;

use eframe::egui;
use std::process::ExitCode;

fn main() -> ExitCode {
    let initial_pdf = match cli::parse(std::env::args_os().skip(1)) {
        Ok(cli::Cli::Run(pdf)) => pdf,
        Ok(cli::Cli::Help) => {
            print!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(cli::Cli::Version) => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("error: {err}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    match eframe::run_native(
        "Typewriter PDF",
        opts,
        Box::new(move |cc| Ok(Box::new(app::TypewriterApp::new(cc, initial_pdf)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
