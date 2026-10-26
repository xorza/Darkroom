//! `darkroom-mcp`: the darkroom editor as an MCP server over stdio, for an agent to open
//! projects in, click through and look at. `darkroom/AGENTS.md`, "The editor harness", holds the
//! decisions.

mod darkroom_server;
mod params;
mod session_thread;

use std::process::ExitCode;
use std::str::FromStr;

use clap::{Parser, ValueEnum};
use darkroom::internals::harness::{DisplayKind, SessionConfig};
use glam::UVec2;
use rmcp::ServiceExt as _;
use rmcp::transport::stdio;

use crate::darkroom_server::DarkroomServer;

/// The darkroom editor as an MCP server over stdio.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Where frames go: `gpu` paints pixels, which shots need, and exits with no adapter; `cpu`
    /// lays out and hit-tests without any.
    #[arg(long, value_enum, default_value_t = DisplayArg::Gpu)]
    display: DisplayArg,
    /// The window's physical size, as `1600x1000`, unless `darkroom_open` names one.
    #[arg(long, default_value = "1600x1000")]
    size: WindowSize,
    /// Physical pixels per logical pixel, unless `darkroom_open` names one.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DisplayArg {
    Gpu,
    Cpu,
}

/// A `WxH` size in pixels.
#[derive(Clone, Copy, Debug)]
struct WindowSize(UVec2);

impl FromStr for WindowSize {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || format!("'{text}' is not a size; write it as 1600x1000");
        let (w, h) = text.split_once('x').ok_or_else(bad)?;
        Ok(Self(UVec2::new(
            w.parse().map_err(|_| bad())?,
            h.parse().map_err(|_| bad())?,
        )))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    let server = DarkroomServer::new(SessionConfig {
        display: match args.display {
            DisplayArg::Gpu => DisplayKind::Gpu,
            DisplayArg::Cpu => DisplayKind::Cpu,
        },
        size: args.size.0,
        scale: args.scale,
    });
    let served = match server.serve(stdio()).await {
        Ok(service) => service
            .waiting()
            .await
            .map(drop)
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // stdout carries the protocol; stderr is the client's log of the server.
            #[expect(
                clippy::print_stderr,
                reason = "the server's one way to report its end"
            )]
            {
                eprintln!("darkroom-mcp: {error}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
