mod app;
mod document;
mod edit;
mod gui;
mod io;
mod node_values;
mod run_state;
mod scene;
mod script;
mod theme;

use clap::Parser;
use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;
use crate::script::ScriptArgs;

/// darkroom — node-graph editor. Optional flags configure the
/// scripting-over-TCP listener (off unless `--script-tcp`).
#[derive(Parser, Debug)]
#[command(version, about = "darkroom node-graph editor")]
struct Cli {
    #[command(flatten)]
    script: ScriptArgs,
}

fn main() {
    init_tracing();

    let cli = Cli::parse();
    let script_cfg = cli.script.to_config();
    WinitHost::new(WinitHostConfig::new("darkroom"), move |ui, handle| {
        App::new(ui, handle, script_cfg)
    })
    .run();
}

/// Minimal stderr tracing subscriber, `RUST_LOG`-controlled (defaults to
/// `info`). `try_init` is a no-op if a subscriber is already installed.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
