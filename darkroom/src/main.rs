// Release builds link as a Windows GUI binary, so double-clicking the .exe in
// Explorer opens the window alone instead of trailing a console behind it.
// Dev builds keep the console subsystem — that is where `init_tracing`'s
// stderr output and clap's `--help` land. Gated on the same `debug_assertions`
// signal as `common::is_debug`; `release-max` inherits `release`, so it has it
// off and gets the GUI subsystem.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod gui;
mod platform;

use std::path::PathBuf;

use clap::Parser;
use common::is_debug;
use palantir::{Image, WindowConfig, WinitHost, WinitHostError};

use crate::core::io::preferences::Preferences;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::App;

/// darkroom — node-graph editor.
#[derive(Parser, Debug)]
#[command(version, about = "darkroom node-graph editor")]
struct Cli {
    /// Document to open, in place of the remembered one. Saving it is what
    /// makes the next bare launch reopen it.
    #[arg(value_name = "FILE")]
    document: Option<PathBuf>,
}

fn main() {
    init_tracing();

    let Cli { document } = Cli::parse();
    if let Err(error) = run_gui(document) {
        tracing::error!("darkroom: {error}");
        std::process::exit(1);
    }
}

/// Launch the Palantir desktop editor on `document` (the command-line
/// argument, if there was one). The winit event loop owns the main thread, so
/// this doesn't return until the window closes.
fn run_gui(document: Option<PathBuf>) -> Result<(), WinitHostError> {
    // Load preferences here, before the window exists, so a saved size /
    // position seeds the window at creation (`App::new` runs after the
    // first window is already up, too late to size it). Reuse the same
    // instance for the app so we don't read the file twice.
    let preferences = Preferences::load();
    // Reverse-DNS, and exactly the basename of
    // `assets/linux/com.cssodessa.darkroom.desktop` — that equality is what
    // ties the window to its launcher entry, so the shell groups the two and
    // takes the taskbar icon from the entry. Wayland windows carry no identity
    // at all without it.
    let mut window = WindowConfig::new("Darkroom").app_id("com.cssodessa.darkroom");
    if let Some(icon) = load_icon() {
        window = window.icon(icon);
    }
    if let Some(w) = &preferences.window {
        window = window.inner_size(w.size).maximized(w.maximized);
        if let Some(pos) = w.position {
            window = window.position(pos);
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

            App::new(ui, handle, preferences, document)
        })?;

    // Between `build` and `run` because that is the only window that works on
    // macOS; every other platform has nothing to do here.
    platform::route_opened_documents(host.handle());
    host.run()
}

/// Decode the baked-in window icon (PNG → RGBA8) for the title bar /
/// taskbar. Honored on Windows and Linux; macOS ignores per-window icons —
/// its Dock icon comes from the `.app` bundle (`[package.metadata.bundle]`
/// in `Cargo.toml`). Returns `None` if the embedded asset ever fails to
/// decode, degrading to the platform default rather than blocking startup.
fn load_icon() -> Option<Image> {
    static ICON_PNG: &[u8] = include_bytes!("../assets/icons/darkroom-256.png");
    let rgba = image::load_from_memory(ICON_PNG)
        .inspect_err(|e| tracing::warn!("window icon decode failed: {e}"))
        .ok()?
        .to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(Image::from_rgba8(w, h, rgba.into_raw()))
}

/// Minimal stderr tracing subscriber, `RUST_LOG`-controlled (defaults to
/// `info`). `try_init` is a no-op if a subscriber is already installed.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_icon_decodes_embedded_png() {
        let rgba = image::load_from_memory(include_bytes!("../assets/icons/darkroom-256.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(rgba.dimensions(), (256, 256));
        assert_eq!(rgba.as_raw().len(), 256 * 256 * 4);
        assert!(load_icon().is_some());
    }

    #[test]
    fn bare_invocation_parses() {
        let cli = Cli::try_parse_from(["darkroom"]).unwrap();
        assert_eq!(cli.document, None);
        assert!(
            Cli::try_parse_from(["darkroom", "--nope"]).is_err(),
            "an unknown flag is still rejected"
        );
    }

    #[test]
    fn a_document_argument_parses() {
        let cli = Cli::try_parse_from(["darkroom", "graphs/scene.darkroom"]).unwrap();
        assert_eq!(cli.document, Some(PathBuf::from("graphs/scene.darkroom")));
        assert!(
            Cli::try_parse_from(["darkroom", "one.darkroom", "two.darkroom"]).is_err(),
            "only one document opens per launch"
        );
    }
}
