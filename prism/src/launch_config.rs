//! Runtime configuration assembled from CLI flags + persisted prefs and
//! handed to the chosen frontend. Two-copy split: `saved` is the
//! `config.toml` snapshot Session writes back; `actual` is `saved` with
//! CLI overrides applied for the running app. CLI flags always win for
//! this run but never round-trip to disk.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use clap::Args;
use uuid::Uuid;

use crate::config::Config;
use crate::script::TcpScriptConfig;

/// Default bind for the TCP script listener when neither CLI nor
/// persisted config specify one. Loopback at a fixed port so a client
/// can connect without a discovery file.
const DEFAULT_SCRIPT_PORT: u16 = 33433;

#[derive(Debug)]
pub struct LaunchConfig {
    /// On-disk preferences as loaded. Session takes ownership and is
    /// the only writer back to the file. Per-launch CLI flags never
    /// touch this.
    pub(crate) saved: Config,
    /// Effective config for this run: `saved` with CLI overrides
    /// applied per-field. Drives transport startup and one-shot
    /// launch decisions like `load_last`. Discarded on exit.
    pub(crate) actual: Config,
}

impl LaunchConfig {
    /// Load `config.toml` and apply CLI overrides to produce the
    /// per-run effective config. Generates a fresh token only as a
    /// fallback when auth is on but neither CLI nor persisted config
    /// supplied one.
    pub fn new(cli: &LaunchCliArgs) -> Self {
        let saved = Config::load_or_default();
        let actual = apply_cli_overrides(&saved, cli, Uuid::new_v4());
        Self { saved, actual }
    }
}

/// All CLI flags that override persisted `Config` for a single run.
/// Flattened into the top-level `Cli` in `main`. `apply_cli_overrides`
/// reads this directly, so `main` doesn't have to unpack the fields.
#[derive(Debug, Default, Args)]
pub struct LaunchCliArgs {
    #[command(flatten)]
    pub script: ScriptCliArgs,

    /// Reopen the graph from the last clean shutdown for this run.
    /// ORed with the persisted `load_last` preference; never written
    /// back to disk.
    #[arg(long, global = true)]
    pub load_last: bool,
}

// Raw CLI flags for the scripting surface. Each Option-typed field
// encodes "user did/didn't pass it" — `None` means leave the persisted
// value alone.
#[derive(Debug, Default, Args)]
pub struct ScriptCliArgs {
    /// Enable the loopback TCP script listener for this run. Without
    /// this flag the persisted `script_tcp` (if any) is used as-is.
    #[arg(long, global = true)]
    pub script_tcp: bool,

    /// Bind spec for the TCP script listener. Accepts a bare port
    /// (`8080`, `:8080`), a full `addr:port` (`127.0.0.1:8080`,
    /// `0.0.0.0:0`), or bracketed IPv6 (`[::1]:8080`). Port `0` lets
    /// the OS pick a free port.
    #[arg(
        long,
        value_name = "BIND",
        value_parser = parse_bind_spec,
        global = true,
        requires = "script_tcp",
    )]
    pub script_bind: Option<SocketAddr>,

    /// Write a JSON `{ "port": N, "token": "..." }` discovery file at
    /// startup so a separately-launched client can find the listener.
    /// Treat the file as a secret.
    #[arg(
        long,
        value_name = "PATH",
        global = true,
        requires = "script_tcp",
        conflicts_with = "script_no_auth"
    )]
    pub script_token_file: Option<PathBuf>,

    /// Use this UUID as the auth token instead of generating a random
    /// one. Convenient for shared-launch scripts where both client and
    /// server already know the token. Still a secret — don't commit it.
    #[arg(
        long,
        value_name = "UUID",
        global = true,
        requires = "script_tcp",
        conflicts_with = "script_no_auth"
    )]
    pub script_token: Option<Uuid>,

    /// Skip token authentication on the TCP script listener. Loopback
    /// only — any local process can connect. Not recommended.
    #[arg(long, global = true, requires = "script_tcp")]
    pub script_no_auth: bool,
}

fn default_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_SCRIPT_PORT)
}

/// Parse `--script-bind` values. Both address and port are optional, with
/// defaults 127.0.0.1 and `DEFAULT_SCRIPT_PORT` respectively. Accepts:
/// - bare port: `"8080"` or `":8080"` → `127.0.0.1:8080`
/// - bare address: `"0.0.0.0"`, `"::1"` → `<addr>:DEFAULT_SCRIPT_PORT`
/// - full socket addr: `"127.0.0.1:8080"`, `"[::1]:8080"`
fn parse_bind_spec(s: &str) -> Result<SocketAddr, String> {
    const DEFAULT_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    // Bare port: "8080" or ":8080". `strip_prefix` (not
    // `trim_start_matches`) so "::1" isn't collapsed to "1".
    let port_candidate = s.strip_prefix(':').unwrap_or(s);
    if let Ok(port) = port_candidate.parse::<u16>() {
        return Ok(SocketAddr::new(DEFAULT_IP, port));
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_SCRIPT_PORT));
    }
    s.parse::<SocketAddr>()
        .map_err(|e| format!("invalid bind spec {s:?}: {e}"))
}

/// Apply CLI overrides on top of `saved` to produce the effective
/// config for this run. CLI fields always win where set; unset fields
/// inherit from `saved`. `--script-tcp` enables the listener for the
/// run even when persisted config has it off, seeding a token from
/// `fresh_token` when none is otherwise available. `cli_load_last` is
/// ORed into `actual.load_last` — the CLI flag can enable for a single
/// run but cannot turn off a persisted preference.
fn apply_cli_overrides(saved: &Config, cli: &LaunchCliArgs, fresh_token: Uuid) -> Config {
    let mut actual = saved.clone();
    actual.load_last = saved.load_last || cli.load_last;

    let script = &cli.script;
    let want_tcp = script.script_tcp || saved.script_tcp.is_some();
    if !want_tcp {
        return actual;
    }

    // Baseline: persisted config (if any) → CLI implicit defaults.
    // For a brand-new listener with auth on, `fresh_token` seeds the
    // baseline so an unset `--script-token` still produces auth.
    let mut tcp = saved.script_tcp.clone().unwrap_or_else(|| TcpScriptConfig {
        bind: default_bind(),
        token: Some(fresh_token),
        token_file: None,
    });

    if let Some(bind) = script.script_bind {
        tcp.bind = bind;
    }
    if let Some(file) = script.script_token_file.clone() {
        tcp.token_file = Some(file);
    }
    if script.script_no_auth {
        // `conflicts_with` already prevents `--script-token` here.
        tcp.token = None;
    } else if let Some(token) = script.script_token {
        tcp.token = Some(token);
    }

    actual.script_tcp = Some(tcp);
    actual
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_token() -> Uuid {
        Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788)
    }

    fn saved_with_tcp(tcp: TcpScriptConfig) -> Config {
        Config {
            script_tcp: Some(tcp),
            ..Config::default()
        }
    }

    fn cli(script: ScriptCliArgs, load_last: bool) -> LaunchCliArgs {
        LaunchCliArgs { script, load_last }
    }

    #[test]
    fn no_cli_no_saved_yields_no_listener() {
        let actual = apply_cli_overrides(
            &Config::default(),
            &cli(ScriptCliArgs::default(), false),
            fixed_token(),
        );
        assert!(actual.script_tcp.is_none());
    }

    #[test]
    fn cli_script_tcp_alone_enables_with_default_bind_and_fresh_token() {
        let args = ScriptCliArgs {
            script_tcp: true,
            ..Default::default()
        };
        let actual = apply_cli_overrides(&Config::default(), &cli(args, false), fixed_token());
        let tcp = actual.script_tcp.expect("listener should be enabled");
        assert_eq!(tcp.bind, default_bind());
        assert_eq!(tcp.token, Some(fixed_token()));
        assert!(tcp.token_file.is_none());
    }

    #[test]
    fn saved_alone_drives_listener_when_cli_silent() {
        let persisted = TcpScriptConfig {
            bind: "0.0.0.0:54321".parse().unwrap(),
            token: Some(Uuid::from_u128(0xabc)),
            token_file: Some(PathBuf::from("/tmp/discovery.json")),
        };
        let actual = apply_cli_overrides(
            &saved_with_tcp(persisted.clone()),
            &cli(ScriptCliArgs::default(), false),
            fixed_token(),
        );
        let tcp = actual.script_tcp.expect("listener should be enabled");
        assert_eq!(tcp.bind, persisted.bind);
        assert_eq!(tcp.token, persisted.token);
        assert_eq!(tcp.token_file, persisted.token_file);
    }

    #[test]
    fn cli_bind_overrides_saved_bind() {
        let saved = saved_with_tcp(TcpScriptConfig {
            bind: "127.0.0.1:8080".parse().unwrap(),
            token: Some(Uuid::from_u128(0xabc)),
            token_file: None,
        });
        let args = ScriptCliArgs {
            script_tcp: true,
            script_bind: Some("0.0.0.0:9090".parse().unwrap()),
            ..Default::default()
        };
        let actual = apply_cli_overrides(&saved, &cli(args, false), fixed_token());
        let tcp = actual.script_tcp.unwrap();
        assert_eq!(tcp.bind, "0.0.0.0:9090".parse::<SocketAddr>().unwrap());
        // Token preserved from saved, not regenerated.
        assert_eq!(tcp.token, Some(Uuid::from_u128(0xabc)));
    }

    #[test]
    fn cli_token_overrides_saved_token() {
        let saved = saved_with_tcp(TcpScriptConfig {
            bind: default_bind(),
            token: Some(Uuid::from_u128(0xabc)),
            token_file: None,
        });
        let explicit = Uuid::from_u128(0xdead);
        let args = ScriptCliArgs {
            script_tcp: true,
            script_token: Some(explicit),
            ..Default::default()
        };
        let actual = apply_cli_overrides(&saved, &cli(args, false), fixed_token());
        assert_eq!(actual.script_tcp.unwrap().token, Some(explicit));
    }

    #[test]
    fn cli_no_auth_clears_saved_token() {
        let saved = saved_with_tcp(TcpScriptConfig {
            bind: default_bind(),
            token: Some(Uuid::from_u128(0xabc)),
            token_file: None,
        });
        let args = ScriptCliArgs {
            script_tcp: true,
            script_no_auth: true,
            ..Default::default()
        };
        let actual = apply_cli_overrides(&saved, &cli(args, false), fixed_token());
        assert_eq!(actual.script_tcp.unwrap().token, None);
    }

    #[test]
    fn cli_token_file_overrides_saved() {
        let saved = saved_with_tcp(TcpScriptConfig {
            bind: default_bind(),
            token: Some(Uuid::from_u128(0xabc)),
            token_file: Some(PathBuf::from("/tmp/old.json")),
        });
        let args = ScriptCliArgs {
            script_tcp: true,
            script_token_file: Some(PathBuf::from("/tmp/new.json")),
            ..Default::default()
        };
        let actual = apply_cli_overrides(&saved, &cli(args, false), fixed_token());
        assert_eq!(
            actual.script_tcp.unwrap().token_file,
            Some(PathBuf::from("/tmp/new.json"))
        );
    }

    #[test]
    fn current_path_passes_through() {
        let saved = Config {
            current_path: Some(PathBuf::from("/tmp/graph.toml")),
            ..Config::default()
        };
        let actual =
            apply_cli_overrides(&saved, &cli(ScriptCliArgs::default(), false), fixed_token());
        assert_eq!(actual.current_path, saved.current_path);
    }

    #[test]
    fn cli_load_last_enables_for_run_only() {
        let saved = Config::default();
        let actual =
            apply_cli_overrides(&saved, &cli(ScriptCliArgs::default(), true), fixed_token());
        assert!(actual.load_last);
        // saved untouched — not persisted by `apply_cli_overrides`.
        assert!(!saved.load_last);
    }

    #[test]
    fn cli_load_last_does_not_disable_persisted() {
        let saved = Config {
            load_last: true,
            ..Config::default()
        };
        let actual =
            apply_cli_overrides(&saved, &cli(ScriptCliArgs::default(), false), fixed_token());
        assert!(actual.load_last);
    }

    #[test]
    fn parse_bind_spec_bare_port() {
        assert_eq!(
            parse_bind_spec("8080").unwrap(),
            "127.0.0.1:8080".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn parse_bind_spec_leading_colon_port() {
        assert_eq!(
            parse_bind_spec(":8080").unwrap(),
            "127.0.0.1:8080".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn parse_bind_spec_full_v4() {
        assert_eq!(
            parse_bind_spec("0.0.0.0:0").unwrap(),
            "0.0.0.0:0".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn parse_bind_spec_full_v6() {
        assert_eq!(
            parse_bind_spec("[::1]:9000").unwrap(),
            "[::1]:9000".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn parse_bind_spec_bare_ipv4_defaults_port() {
        assert_eq!(
            parse_bind_spec("0.0.0.0").unwrap(),
            SocketAddr::new("0.0.0.0".parse().unwrap(), DEFAULT_SCRIPT_PORT)
        );
    }

    #[test]
    fn parse_bind_spec_bare_ipv6_defaults_port() {
        assert_eq!(
            parse_bind_spec("::1").unwrap(),
            SocketAddr::new("::1".parse().unwrap(), DEFAULT_SCRIPT_PORT)
        );
    }

    #[test]
    fn parse_bind_spec_rejects_garbage() {
        assert!(parse_bind_spec("not-an-addr").is_err());
        assert!(parse_bind_spec("127.0.0.1:").is_err());
    }
}
