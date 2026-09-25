//! Aftermission: post-mission review of ArduPilot dataflash logs, built on
//! dflog and egui.

mod app;
mod model;
mod modes;
mod plot;
mod settings;
mod timefmt;
mod tree;
mod worker;

/// Log filter when `RUST_LOG` is unset.
const DEFAULT_LOG: &str = "aftermission=info,wgpu_core=warn,wgpu_hal=error,naga=warn";

fn main() -> eframe::Result {
    use std::path::PathBuf;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG)),
        )
        .init();

    // Optional log to open on start: `aftermission flight.bin`.
    let initial: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Aftermission")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "aftermission",
        options,
        Box::new(move |cc| Ok(Box::new(app::AftermissionApp::new(cc, initial)))),
    )
}
