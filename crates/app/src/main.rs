//! Aftermission: post-mission review of ArduPilot dataflash logs, built on
//! dflog and egui.

// A release build on Windows opens no console window behind the app; a
// debug build keeps it for `RUST_LOG`. Not for the test harness, which
// is built from this file too and would print nothing.
#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

mod app;
mod codes;
mod csv;
#[cfg(any(target_arch = "wasm32", test))]
mod download;
mod events;
mod filter;
mod map;
mod model;
mod modes;
mod paramfile;
mod params;
#[cfg(feature = "parquet")]
mod parquetdir;
#[cfg(any(target_arch = "wasm32", test))]
mod picks;
mod plot;
mod settings;
mod timefmt;
mod tree;
mod worker;

// The Parquet export needs a folder picker and the arrow tree, neither
// of which the browser build has; Trunk.toml turns the feature off.
#[cfg(all(feature = "parquet", target_arch = "wasm32"))]
compile_error!(
    "the `parquet` feature is native only: build for the web with --no-default-features"
);

/// The id eframe files settings under, and the map its tile cache beside.
#[cfg(not(target_arch = "wasm32"))]
pub const APP_ID: &str = "aftermission";

/// Log filter when `RUST_LOG` is unset, and always in the browser.
const DEFAULT_LOG: &str = "aftermission=info,wgpu_core=warn,wgpu_hal=error,naga=warn";

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    use std::io::IsTerminal as _;
    use std::path::PathBuf;

    // Colors for a terminal only: a log redirected to a file, the one way
    // to keep the Windows release build's, stays plain text.
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
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
            .with_icon(icon())
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| Ok(Box::new(app::AftermissionApp::new(cc, initial)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    // No timestamps: the subscriber's clock is `SystemTime::now()`, which
    // panics in the browser.
    tracing_subscriber::fmt()
        .with_writer(web_log::MakeConsoleWriter)
        .with_ansi(false)
        .without_time()
        .with_env_filter(tracing_subscriber::EnvFilter::new(DEFAULT_LOG))
        .init();

    wasm_bindgen_futures::spawn_local(async {
        let window = eframe::web_sys::window().expect("the page runs in a window");
        let canvas = window
            .document()
            .and_then(|d| d.get_element_by_id("aftermission_canvas"))
            .and_then(|e| e.dyn_into::<eframe::web_sys::HtmlCanvasElement>().ok())
            .expect("index.html provides a canvas with id aftermission_canvas");
        let mut options = eframe::WebOptions::default();
        if asks_for_webgl(&window.location().search().unwrap_or_default()) {
            force_webgl(&mut options);
        }
        if let Err(err) = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(|cc| Ok(Box::new(app::AftermissionApp::new(cc)))),
            )
            .await
        {
            tracing::error!(?err, "cannot start the web app");
        }
    });
}

/// The window's icon, painted by `examples/icon.rs`. The executable's own
/// icon is a Windows resource that `build.rs` embeds from the same drawing.
#[cfg(not(target_arch = "wasm32"))]
fn icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon/aftermission.png"))
        .expect("the icon is a PNG the build ships")
}

/// Whether the page's query, `?webgl` or `?x=1&webgl`, asks for WebGL.
#[cfg(any(target_arch = "wasm32", test))]
fn asks_for_webgl(search: &str) -> bool {
    search
        .trim_start_matches('?')
        .split('&')
        .any(|pair| pair.split('=').next() == Some("webgl"))
}

/// Leave WebGPU out of the backends wgpu may pick, so it starts on
/// WebGL 2, as egui-wgpu itself does on a page that is not a secure
/// context. For checking the fallback, and for a browser whose WebGPU
/// draws wrong.
#[cfg(target_arch = "wasm32")]
fn force_webgl(options: &mut eframe::WebOptions) {
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup
            .instance_descriptor
            .backends
            .remove(eframe::wgpu::Backends::BROWSER_WEBGPU);
        tracing::info!("WebGPU left out on request; drawing through WebGL");
    }
}

/// Route `tracing` output to the browser console.
#[cfg(target_arch = "wasm32")]
mod web_log {
    use std::io;

    pub struct MakeConsoleWriter;

    pub struct ConsoleWriter;

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for MakeConsoleWriter {
        type Writer = ConsoleWriter;

        fn make_writer(&'a self) -> Self::Writer {
            ConsoleWriter
        }
    }

    impl io::Write for ConsoleWriter {
        /// The fmt layer writes one event per call.
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let line = String::from_utf8_lossy(buf);
            eframe::web_sys::console::log_1(&line.trim_end().into());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{asks_for_webgl, icon};

    #[test]
    fn the_window_icon_is_the_painted_png() {
        let icon = icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    #[test]
    fn the_address_asks_for_webgl_by_a_bare_or_valued_key() {
        assert!(asks_for_webgl("?webgl"));
        assert!(asks_for_webgl("?webgl=1"));
        assert!(asks_for_webgl("?log=flight.bin&webgl"));
        assert!(!asks_for_webgl(""));
        assert!(!asks_for_webgl("?"));
        assert!(!asks_for_webgl("?webgl2"));
        assert!(!asks_for_webgl("?nowebgl"));
    }
}
