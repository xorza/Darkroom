mod common;
mod config;
mod gui;
mod headless;
mod init;
mod input;
mod launch_config;
mod model;
mod script;
mod session;
mod tui;
mod ui_host;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::launch_config::{LaunchCliArgs, LaunchConfig};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(flatten)]
    overrides: LaunchCliArgs,

    #[command(subcommand)]
    mode: Option<Mode>,
}

#[derive(Debug, Subcommand)]
enum Mode {
    /// Run the egui desktop frontend (default).
    Gui,
    /// Run the terminal frontend.
    Tui,
    /// No UI — run script transports + worker only. Exits on
    /// `shutdown()` from a script or on Ctrl-C.
    Headless,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Hold the log guard for the lifetime of `main` so the non-blocking
    // tracing writer flushes on normal exit.
    let _log_guard = init::init();
    let cli = Cli::parse();

    let launch_config = LaunchConfig::new(&cli.overrides);

    match cli.mode.unwrap_or(Mode::Gui) {
        Mode::Gui => gui::run(launch_config),
        Mode::Tui => tui::run(launch_config).await,
        Mode::Headless => headless::run(launch_config).await,
    }
}
