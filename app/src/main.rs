//! The `darkroom` binary: the editor's window, or the editor served to an agent over MCP, as the
//! command line asks. `darkroom/AGENTS.md`, "The editor harness", holds the decisions.

// One Windows link in every build: a GUI binary, so double-clicking the .exe in Explorer opens the
// window alone, which joins the terminal that started it (`process::attach_parent_console`). A GUI
// binary still takes the pipes a parent hands it, as an MCP client does. Tests stay a console
// binary, as a GUI one hides `cargo test`'s output (rust#67159).
#![cfg_attr(not(test), windows_subsystem = "windows")]
#![deny(unsafe_code)]

mod cli;
mod mcp;

use std::env;
use std::process::ExitCode;

use common::{ErrorChain, process};
use darkroom::driver::log_ring::LogRing;

use crate::cli::{Command, HELP, Launch};

/// What a usage error exits with, as clap's and the POSIX utilities' do.
const USAGE_ERROR: u8 = 2;

#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "help and the version answer on stdout and a usage error on stderr, before any log"
)]
fn main() -> ExitCode {
    process::attach_parent_console();
    let launch = match cli::parse(env::args_os().skip(1)) {
        Ok(Command::Launch(launch)) => launch,
        Ok(Command::Help) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!("darkroom {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("error: {error}\n\nFor more information, try '--help'.");
            return ExitCode::from(USAGE_ERROR);
        }
    };
    match launch {
        Launch::Window(config) => {
            darkroom::init_tracing(None);
            match darkroom::run(config) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    tracing::error!("darkroom: {}", ErrorChain::new(&error));
                    ExitCode::FAILURE
                }
            }
        }
        Launch::McpWindow(config) => {
            let log = LogRing::default();
            darkroom::init_tracing(Some(&log));
            mcp::serve_window(config, log)
        }
        Launch::McpHeadless {
            document,
            size,
            scale,
        } => {
            let log = LogRing::default();
            darkroom::init_tracing(Some(&log));
            mcp::serve_headless(document, size, scale, log)
        }
    }
}
