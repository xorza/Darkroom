mod app_config;
mod common;
mod config;
mod gui;
mod init;
mod input;
mod model;
mod script;
mod session;
mod tui;
mod ui_host;

use anyhow::Result;
use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::app_config::{AppConfig, ScriptCliArgs, build_script_config};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(flatten)]
    script: ScriptCliArgs,

    #[command(subcommand)]
    mode: Option<Mode>,
}

#[derive(Debug, Subcommand)]
enum Mode {
    /// Run the egui desktop frontend (default).
    Gui,
    /// Run the terminal frontend.
    Tui,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Hold the log guard for the lifetime of `main` so the non-blocking
    // tracing writer flushes on normal exit.
    let _log_guard = init::init();
    let cli = Cli::parse();

    let app_config = AppConfig {
        script: build_script_config(&cli.script, Uuid::new_v4()),
    };

    match cli.mode.unwrap_or(Mode::Gui) {
        Mode::Gui => gui::run(app_config),
        Mode::Tui => tui::run(app_config),
    }
}
