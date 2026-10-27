//! The `darkroom` binary: the editor's window, or the editor served to an agent over MCP, as the
//! command line asks. `darkroom/AGENTS.md`, "The editor harness", holds the decisions.

// Release builds link as a Windows GUI binary, so double-clicking the .exe in
// Explorer opens the window alone instead of trailing a console behind it.
// Dev builds keep the console subsystem — that is where `init_tracing`'s
// stderr output and clap's `--help` land. Gated on the same `debug_assertions`
// signal as `common::is_debug`, which `release`, `release-dev` and `bench`
// turn off. A GUI binary still takes the pipes a parent hands it, as an MCP
// client does (rust#67159).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_code)]

mod cli;
mod mcp;

use std::process::ExitCode;

use clap::Parser;
use darkroom::driver::log_ring::LogRing;

use crate::cli::{Cli, Launch};

fn main() -> ExitCode {
    match Cli::parse().launch() {
        Launch::Window { document } => {
            darkroom::init_tracing(None);
            match darkroom::run(document) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    tracing::error!("darkroom: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Launch::McpWindow { document } => {
            let log = LogRing::default();
            darkroom::init_tracing(Some(&log));
            mcp::serve_window(document, log)
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
