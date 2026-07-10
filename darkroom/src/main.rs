mod core;
mod gui;
mod headless;
mod tui;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use aperture::{WindowIcon, WinitHost, WinitHostConfig};
use clap::{Parser, Subcommand};
use common::is_debug;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::core::io::preferences::Preferences;
use crate::core::script::tcp::TcpScriptConfig;
use crate::core::script::{DEFAULT_BIND, ScriptConfig};
use crate::core::session::Session;
use crate::core::wake;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::App;

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
    /// Run the Aperture desktop editor (default).
    Gui,
    /// Run the terminal command shell — a stdin REPL, no graph rendering.
    Tui,
    /// No UI: host the script TCP server + evaluation worker only. Exits on
    /// a script `shutdown()` or Ctrl-C.
    Headless,
}

/// CLI flags configuring the script TCP listener, flattened into [`Cli`].
/// All optional; with no `--script-tcp` the listener stays off and the
/// editor behaves exactly as before.
#[derive(clap::Args, Debug, Default)]
struct ScriptArgs {
    /// Enable the TCP script listener.
    #[arg(long)]
    script_tcp: bool,
    /// Bind address: a bare port (`34567` / `:34567`), an IP (uses the
    /// default port), or a full `host:port`. Defaults to `127.0.0.1:34567`.
    /// A non-loopback bind widens exposure and warns at startup.
    #[arg(long, value_name = "ADDR")]
    script_bind: Option<String>,
    /// Require this 16-byte UUID auth token from every client.
    #[arg(long, value_name = "UUID", conflicts_with = "script_no_auth")]
    script_token: Option<Uuid>,
    /// Accept any client without a handshake (loopback bind still advised).
    /// Mutually exclusive with `--script-token`.
    #[arg(long)]
    script_no_auth: bool,
    /// Write a JSON discovery file (`{"port": N, "token": "..."}`) at
    /// startup so a client can find the address + token.
    #[arg(long, value_name = "PATH")]
    script_token_file: Option<PathBuf>,
}

impl ScriptArgs {
    /// Resolve these flags into a [`ScriptConfig`]: the listener-off default
    /// unless `--script-tcp` is set. When on with neither `--script-token`
    /// nor `--script-no-auth`, a fresh random token is minted so the
    /// listener defaults to authenticated.
    fn to_config(&self) -> ScriptConfig {
        if !self.script_tcp {
            return ScriptConfig::default();
        }
        let bind = match &self.script_bind {
            Some(spec) => parse_bind_spec(spec).unwrap_or_else(|e| {
                tracing::warn!("--script-bind: {e}; falling back to {DEFAULT_BIND}");
                DEFAULT_BIND
            }),
            None => DEFAULT_BIND,
        };
        let token = if self.script_no_auth {
            None
        } else {
            Some(self.script_token.unwrap_or_else(Uuid::new_v4))
        };
        ScriptConfig {
            tcp: Some(TcpScriptConfig {
                bind,
                token,
                token_file: self.script_token_file.clone(),
            }),
        }
    }
}

/// Parse a `--script-bind` spec into a `SocketAddr`: a bare port
/// (`"34567"` / `":34567"`), a bare IP (`"0.0.0.0"`, `"::1"` → default
/// port), or a full socket addr (`"127.0.0.1:8080"`, `"[::1]:8080"`).
fn parse_bind_spec(s: &str) -> Result<SocketAddr, String> {
    // Bare port: "34567" or ":34567". `strip_prefix` (not
    // `trim_start_matches`) so "::1" isn't collapsed to "1".
    let port_candidate = s.strip_prefix(':').unwrap_or(s);
    if let Ok(port) = port_candidate.parse::<u16>() {
        return Ok(SocketAddr::new(DEFAULT_BIND.ip(), port));
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_BIND.port()));
    }
    s.parse::<SocketAddr>()
        .map_err(|e| format!("invalid bind spec {s:?}: {e}"))
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

/// Launch the Aperture desktop editor. The winit event loop owns the main
/// thread, so this doesn't return until the window closes.
fn run_gui(script_cfg: ScriptConfig) {
    // Load preferences here, before the window exists, so a saved size /
    // position seeds the window at creation (`App::new` runs after the
    // first window is already up, too late to size it). Reuse the same
    // instance for the app so we don't read the file twice.
    let preferences = Preferences::load();
    let mut config = WinitHostConfig::new("Darkroom");
    config.window.icon = load_icon();
    if let Some(w) = &preferences.window {
        config.window.inner_size = Some(w.size);
        config.window.position = w.position;
        config.window.maximized = w.maximized;
    }
    WinitHost::new(MAIN_WINDOW, config, move |ui, handle| {
        ui.debug_overlay_mut().damage_rect = is_debug();

        App::new(ui, handle, script_cfg, preferences)
    })
    .run();
}

/// Decode the baked-in window icon (PNG → RGBA8) for the title bar /
/// taskbar. Honored on Windows and Linux; macOS ignores per-window icons —
/// its Dock icon comes from the `.app` bundle (`[package.metadata.bundle]`
/// in `Cargo.toml`). Returns `None` if the embedded asset ever fails to
/// decode, degrading to the platform default rather than blocking startup.
fn load_icon() -> Option<WindowIcon> {
    static ICON_PNG: &[u8] = include_bytes!("../assets/icons/darkroom-256.png");
    let rgba = image::load_from_memory(ICON_PNG)
        .inspect_err(|e| tracing::warn!("window icon decode failed: {e}"))
        .ok()?
        .to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(WindowIcon::from_rgba(rgba.into_raw(), w, h))
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
        tracing::error!("darkroom: {e}");
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
    fn load_icon_decodes_embedded_png() {
        // Guards the runtime window-icon path: the baked asset must decode
        // to 256×256 RGBA8 so `WindowIcon::from_rgba`'s length invariant
        // holds. Catches a swapped-in wrong-size or non-RGBA asset.
        let icon = load_icon().expect("embedded window icon decodes");
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4, "RGBA8 buffer length");
    }

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

    #[test]
    fn parse_bind_spec_variants() {
        use std::net::{Ipv4Addr, Ipv6Addr};
        // Bare port (with and without the leading colon) → loopback.
        assert_eq!(
            parse_bind_spec("34567").unwrap(),
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 34567)
        );
        assert_eq!(
            parse_bind_spec(":8080").unwrap(),
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 8080)
        );
        // Bare IP → default port (and "::1" must survive the colon-strip).
        assert_eq!(
            parse_bind_spec("0.0.0.0").unwrap(),
            SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), DEFAULT_BIND.port())
        );
        assert_eq!(
            parse_bind_spec("::1").unwrap(),
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), DEFAULT_BIND.port())
        );
        // Full socket addr passes through.
        assert_eq!(
            parse_bind_spec("127.0.0.1:9000").unwrap(),
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 9000)
        );
        assert!(parse_bind_spec("not-an-addr").is_err());
    }

    #[test]
    fn script_args_default_disables_listener() {
        assert!(ScriptArgs::default().to_config().tcp.is_none());
    }

    #[test]
    fn script_args_tcp_mints_token_and_uses_default_bind() {
        let cfg = ScriptArgs {
            script_tcp: true,
            ..Default::default()
        }
        .to_config();
        let tcp = cfg.tcp.expect("listener enabled");
        assert!(tcp.token.is_some(), "auth on by default");
        assert_eq!(tcp.bind, DEFAULT_BIND);
    }

    #[test]
    fn script_args_no_auth_clears_token() {
        let cfg = ScriptArgs {
            script_tcp: true,
            script_no_auth: true,
            ..Default::default()
        }
        .to_config();
        assert!(cfg.tcp.unwrap().token.is_none());
    }

    #[test]
    fn script_args_explicit_token_and_bind() {
        let token = Uuid::new_v4();
        let cfg = ScriptArgs {
            script_tcp: true,
            script_bind: Some(":9999".into()),
            script_token: Some(token),
            ..Default::default()
        }
        .to_config();
        let tcp = cfg.tcp.unwrap();
        assert_eq!(tcp.token, Some(token));
        assert_eq!(tcp.bind.port(), 9999);
    }
}
