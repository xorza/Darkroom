mod app;
mod document;
mod edit;
mod func_lib;
mod gui;
mod headless;
mod io;
mod node_values;
mod run_state;
mod scene;
mod script;
mod session;
mod theme;
mod tui;
mod wake;
mod worker;

use std::sync::Arc;

use clap::{Parser, Subcommand};
use palantir::{WinitHost, WinitHostConfig};
use tokio::sync::Notify;

use crate::app::App;
use crate::script::{ScriptArgs, ScriptConfig};
use crate::session::Session;

/// darkroom — node-graph editor. The optional subcommand picks the
/// frontend (default `gui`); the flags configure the scripting-over-TCP
/// listener (off unless `--script-tcp`) and apply to every mode. Put flags
/// before the subcommand: `darkroom --script-tcp headless`.
#[derive(Parser, Debug)]
#[command(version, about = "darkroom node-graph editor")]
struct Cli {
    #[command(flatten)]
    script: ScriptArgs,
    #[command(subcommand)]
    mode: Option<Mode>,
}

/// Which frontend to run. `gui` is the default when no subcommand is given.
#[derive(Subcommand, Debug)]
enum Mode {
    /// Run the Palantir desktop editor (default).
    Gui,
    /// Run the terminal command shell — a stdin REPL, no graph rendering.
    Tui,
    /// No UI: host the script TCP server + evaluation worker only. Exits on
    /// a script `shutdown()` or Ctrl-C.
    Headless,
}

fn main() {
    init_tracing();

    let cli = Cli::parse();
    let script_cfg = cli.script.to_config();
    match cli.mode.unwrap_or(Mode::Gui) {
        Mode::Gui => run_gui(script_cfg),
        Mode::Tui => run_terminal(Frontend::Tui, script_cfg),
        Mode::Headless => run_terminal(Frontend::Headless, script_cfg),
    }
}

/// Launch the Palantir desktop editor. The winit event loop owns the main
/// thread, so this doesn't return until the window closes.
fn run_gui(script_cfg: ScriptConfig) {
    WinitHost::new(WinitHostConfig::new("darkroom"), move |ui, handle| {
        App::new(ui, handle, script_cfg)
    })
    .run();
}

/// The non-GUI frontends, dispatched by [`run_terminal`].
enum Frontend {
    Tui,
    Headless,
}

/// Build the [`Session`] in this (sync) context, run the chosen frontend's
/// async loop on a current-thread runtime, then drop everything here.
/// `Session` owns the worker/script tokio runtimes, and dropping a runtime
/// is only allowed *outside* an async context — so it must outlive the
/// `block_on` rather than be dropped inside the driver future.
fn run_terminal(frontend: Frontend, script_cfg: ScriptConfig) {
    if matches!(frontend, Frontend::Headless) && script_cfg.tcp.is_none() {
        tracing::warn!(
            "headless with no script listener — pass --script-tcp so a client can drive it (Ctrl-C to exit)"
        );
    }

    let notify = Arc::new(Notify::new());
    let mut session = Session::new(&script_cfg, wake::from_notify(notify.clone()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build async runtime");
    let result = match frontend {
        Frontend::Tui => runtime.block_on(tui::run(&mut session, &notify)),
        Frontend::Headless => runtime.block_on(headless::run(&mut session, &notify)),
    };
    // `runtime` + `session` (→ the worker/script runtimes) drop at the end
    // of this fn, back in sync context — safe.
    if let Err(e) = result {
        eprintln!("darkroom: {e}");
        std::process::exit(1);
    }
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
    fn no_subcommand_defaults_to_gui() {
        let cli = Cli::try_parse_from(["darkroom"]).unwrap();
        assert!(
            cli.mode.is_none(),
            "no subcommand resolves to the gui default"
        );
    }

    #[test]
    fn parses_mode_subcommands() {
        assert!(matches!(
            Cli::try_parse_from(["darkroom", "tui"]).unwrap().mode,
            Some(Mode::Tui)
        ));
        assert!(matches!(
            Cli::try_parse_from(["darkroom", "headless"]).unwrap().mode,
            Some(Mode::Headless)
        ));
    }

    #[test]
    fn script_flags_precede_subcommand() {
        let cli = Cli::try_parse_from(["darkroom", "--script-tcp", "headless"]).unwrap();
        assert!(matches!(cli.mode, Some(Mode::Headless)));
        assert!(
            cli.script.to_config().tcp.is_some(),
            "the listener flag applies alongside the subcommand"
        );
    }
}
