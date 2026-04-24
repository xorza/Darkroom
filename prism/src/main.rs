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

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
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
    init::init()?;
    let cli = Cli::parse();

    match cli.mode.unwrap_or(Mode::Gui) {
        Mode::Gui => gui::run(),
        Mode::Tui => tui::run(),
    }
}
