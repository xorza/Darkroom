//! Darkroom, the node-graph editor: the editor's core and GUI, which the `darkroom` binary
//! launches through [`run`].

#![deny(unsafe_code)]

mod core;
pub mod driver;
mod gui;
mod platform;

use std::io;
use std::path::PathBuf;

use glam::UVec2;
use palantir::{DebugOverlayConfig, Image, WindowConfig, WinitHost, WinitHostError};
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::driver::log_ring::LogRing;
use crate::driver::window_link::WindowLink;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::{App, LaunchInputs};
use crate::gui::dialogs::{DialogFallback, Dialogs, NativeDialogFlag};

pub use crate::gui::dialogs::DialogAnswer;

/// What a window launch opens and shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunConfig {
    /// The command-line argument, if there was one.
    pub document: Option<PathBuf>,
    /// The diagnostics painted over the editor; none by default, in every build.
    pub overlay: DebugOverlayConfig,
}

/// Launch the Palantir desktop editor. The winit event loop owns the main thread, so this doesn't
/// return until the window closes.
pub fn run(config: RunConfig) -> Result<(), WinitHostError> {
    run_with(config, PreferencesStore::user(), |_| {})
}

/// [`run`], handing `serve` the link a driver on another thread reaches the window through, on
/// the main thread once the app exists and before its first frame. Its preferences are the
/// defaults, kept in memory, as an offscreen driver's are: `darkroom/AGENTS.md`, "The editor
/// harness", "A driven session leaves the preferences file alone".
pub fn run_driven(
    config: RunConfig,
    serve: impl FnOnce(WindowLink) + 'static,
) -> Result<(), WinitHostError> {
    run_with(config, PreferencesStore::Memory, serve)
}

/// The window on the preferences `store` holds, its link handed to `serve`.
fn run_with(
    config: RunConfig,
    store: PreferencesStore,
    serve: impl FnOnce(WindowLink) + 'static,
) -> Result<(), WinitHostError> {
    let RunConfig { document, overlay } = config;
    // Before the window exists, so a saved size and position seed it at creation; `App::new` runs
    // after the window is up, too late to size it.
    let preferences = store.open();
    // Reverse-DNS, and exactly the basename of `packaging/linux/com.cssodessa.darkroom.desktop`:
    // that equality ties the window to its launcher entry, so the shell groups the two and takes
    // the taskbar icon from the entry. A Wayland window has no identity without it.
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
            ui.set_debug_overlay(overlay);

            let native_dialog = NativeDialogFlag::default();
            let dialogs = Dialogs::new(DialogFallback::Native(native_dialog.clone()));
            let app = App::new(
                ui,
                LaunchInputs {
                    preferences,
                    document,
                    dialogs,
                },
            );
            serve(WindowLink::new(
                handle,
                native_dialog,
                app.quit_flag().clone(),
            ));
            app
        })?;

    // Between `build` and `run`: the only point that works on macOS.
    platform::route_opened_documents(host.handle(), App::open_document_at);
    host.run()
}

/// The baked-in window icon. macOS ignores per-window icons: its Dock icon comes from the `.app`
/// bundle (`[package.metadata.bundle]` in `Cargo.toml`).
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
/// (<https://modelcontextprotocol.io/specification/2025-06-18/basic/transports>). Then logs how
/// LibRaw was built, which its build script leaves out of the build output.
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
    tracing::info!(openmp = libraw::OPENMP, "libraw tiled decoders");
}

/// `RUST_LOG`, or `info` without it.
fn log_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

#[cfg(feature = "internals")]
pub mod bench;

#[cfg(feature = "internals")]
pub mod internals;

/// The allocator the record-path allocation gate counts through.
#[cfg(test)]
mod counting_allocator {
    use common::internals::alloc_audit::CountingAllocator;

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
}

#[cfg(test)]
mod tests {
    use glam::UVec2;

    use crate::load_icon;

    #[test]
    fn load_icon_decodes_embedded_png() {
        assert_eq!(load_icon().size(), UVec2::new(256, 256));
    }
}
