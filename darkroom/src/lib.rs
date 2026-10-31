//! Darkroom, the node-graph editor: the editor's core and GUI, which the `darkroom` binary
//! launches through [`run`].

#![deny(unsafe_code)]

mod core;
pub mod driver;
mod gui;
mod platform;

use std::io;
use std::path::PathBuf;

use common::is_debug;
use glam::UVec2;
use palantir::{Image, WindowConfig, WinitHost, WinitHostError};
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::driver::log_ring::LogRing;
use crate::driver::window_link::WindowLink;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::{App, LaunchInputs};
use crate::gui::dialogs::{DialogFallback, Dialogs};

pub use crate::gui::dialogs::DialogAnswer;

/// Launch the Palantir desktop editor on `document` (the command-line
/// argument, if there was one). The winit event loop owns the main thread, so
/// this doesn't return until the window closes.
pub fn run(document: Option<PathBuf>) -> Result<(), WinitHostError> {
    run_driven(document, |_| {})
}

/// [`run`], handing `serve` the link a driver on another thread reaches the window through, on
/// the main thread once the app exists and before its first frame.
pub fn run_driven(
    document: Option<PathBuf>,
    serve: impl FnOnce(WindowLink) + 'static,
) -> Result<(), WinitHostError> {
    // Load preferences here, before the window exists, so a saved size /
    // position seeds the window at creation (`App::new` runs after the
    // first window is already up, too late to size it). Reuse the same
    // instance for the app so we don't read the file twice.
    let preferences = PreferencesStore::user().open();
    // Reverse-DNS, and exactly the basename of
    // `packaging/linux/com.cssodessa.darkroom.desktop` — that equality is what
    // ties the window to its launcher entry, so the shell groups the two and
    // takes the taskbar icon from the entry. Wayland windows carry no identity
    // at all without it.
    let mut window = WindowConfig::new("Darkroom")
        .with_app_id("com.cssodessa.darkroom")
        .with_icon(load_icon());
    if let Some(w) = &preferences.preferences.window {
        window = window.with_inner_size(w.size).with_maximized(w.maximized);
        if let Some(pos) = w.physical_pos {
            window = window.with_physical_pos(pos);
        }
    }
    let host = WinitHost::builder(MAIN_WINDOW)
        .window(window)
        .build(move |ui, handle| {
            let mut overlay = ui.debug_overlay();
            overlay.damage_rect = is_debug();
            // The `settle n/m` half of this readout is how a gesture's
            // record-pass cost gets read in the running editor — drag a
            // node and watch whether both halves advance together.
            overlay.frame_stats = is_debug();
            ui.set_debug_overlay(overlay);

            let dialogs = Dialogs::new(DialogFallback::Native);
            let native_open = dialogs.native_open();
            let app = App::new(
                ui,
                LaunchInputs {
                    preferences,
                    document,
                    dialogs,
                },
            );
            serve(WindowLink::new(handle, native_open));
            app
        })?;

    // Between `build` and `run` because that is the only window that works on
    // macOS; every other platform has nothing to do here.
    platform::route_opened_documents(host.handle(), App::open_document_at);
    host.run()
}

/// Decode the baked-in window icon (PNG → RGBA8) for the title bar /
/// taskbar. Honored on Windows and Linux; macOS ignores per-window icons —
/// its Dock icon comes from the `.app` bundle (`[package.metadata.bundle]`
/// in `Cargo.toml`).
fn load_icon() -> Image {
    static ICON_PNG: &[u8] = include_bytes!("../assets/icons/darkroom-256.png");
    let rgba = image::load_from_memory(ICON_PNG)
        .expect("the embedded icon decodes, as a test holds")
        .to_rgba8();
    let (w, h) = rgba.dimensions();
    Image::from_srgba8(UVec2::new(w, h), rgba.into_raw()).expect("the embedded icon is RGBA8")
}

/// The process's tracing: `RUST_LOG`-filtered lines on stderr (`info` without it), and the
/// warnings and errors of every thread in `ring`, for a driver to report. `try_init` is a no-op
/// if a subscriber is already installed. Stderr in every mode: MCP's stdio transport forbids any
/// byte on stdout but its messages and allows logs on stderr
/// (<https://modelcontextprotocol.io/specification/2025-06-18/basic/transports>).
pub fn init_tracing(ring: Option<&LogRing>) {
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(io::stderr)
                .with_filter(log_filter()),
        )
        .with(ring.map(LogRing::layer))
        .try_init()
        .ok();
}

/// `RUST_LOG`, or `info` without it.
fn log_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

#[cfg(feature = "internals")]
pub mod bench;

#[cfg(feature = "internals")]
pub mod internals;

#[cfg(test)]
#[expect(
    unsafe_code,
    reason = "a `GlobalAlloc` is an `unsafe` trait; the audit counts allocations"
)]
mod alloc_audit;

#[cfg(test)]
mod tests {
    use glam::UVec2;

    use crate::load_icon;

    #[test]
    fn load_icon_decodes_embedded_png() {
        assert_eq!(load_icon().size(), UVec2::new(256, 256));
    }
}
