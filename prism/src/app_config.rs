//! Runtime configuration assembled from CLI flags and handed to the
//! chosen frontend. A single aggregate keeps frontend constructors
//! future-proof: new runtime knobs land as fields here rather than as
//! additional arguments.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use clap::Args;
use uuid::Uuid;

use crate::script::{ScriptConfig, TcpScriptConfig};

/// Default bind port for the TCP script listener when `--script-bind` is
/// absent or specifies only an address. Fixed so a client (e.g. a CLI
/// wrapper) can connect without reading a discovery file. Pick a fresh
/// port if two prism instances need to coexist.
pub const DEFAULT_SCRIPT_PORT: u16 = 33433;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub script: ScriptConfig,
}

// Raw CLI flags for the scripting surface. Flattened into the top-level
// `Cli` via `#[command(flatten)]`. Kept as its own struct so
// `build_script_config` can be unit-tested without going through clap.
#[derive(Debug, Args)]
pub struct ScriptCliArgs {
    /// Enable the loopback TCP script listener (off by default).
    #[arg(long, global = true)]
    pub script_tcp: bool,

    /// Bind spec for the TCP script listener. Accepts a bare port
    /// (`8080`, `:8080`), a full `addr:port` (`127.0.0.1:8080`,
    /// `0.0.0.0:0`), or bracketed IPv6 (`[::1]:8080`). Port `0` lets
    /// the OS pick a free port. A non-loopback address emits a warning
    /// at startup — binding beyond 127.0.0.1 exposes the listener to
    /// other hosts on the network.
    #[arg(
        long,
        value_name = "BIND",
        value_parser = parse_bind_spec,
        default_value = "127.0.0.1:33433",
        global = true,
        requires = "script_tcp",
    )]
    pub script_bind: SocketAddr,

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

impl Default for ScriptCliArgs {
    fn default() -> Self {
        Self {
            script_tcp: false,
            script_bind: default_bind(),
            script_token_file: None,
            script_token: None,
            script_no_auth: false,
        }
    }
}

fn default_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_SCRIPT_PORT)
}

/// Parse `--script-bind` values. Both address and port are optional, with
/// defaults 127.0.0.1 and 0 (OS-assigned) respectively. Accepts:
/// - bare port: `"8080"` or `":8080"` → `127.0.0.1:8080`
/// - bare address: `"0.0.0.0"`, `"::1"` → `<addr>:0`
/// - full socket addr: `"127.0.0.1:8080"`, `"[::1]:8080"`
fn parse_bind_spec(s: &str) -> Result<SocketAddr, String> {
    const DEFAULT_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    // Bare port: "8080" or ":8080". Use `strip_prefix` (not
    // `trim_start_matches`) so "::1" isn't collapsed to "1".
    let port_candidate = s.strip_prefix(':').unwrap_or(s);
    if let Ok(port) = port_candidate.parse::<u16>() {
        return Ok(SocketAddr::new(DEFAULT_IP, port));
    }
    // Bare address (no port) falls back to the default port.
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_SCRIPT_PORT));
    }
    // Full addr:port, including bracketed IPv6.
    s.parse::<SocketAddr>()
        .map_err(|e| format!("invalid bind spec {s:?}: {e}"))
}

/// Build a [`ScriptConfig`] from parsed CLI flags. `fresh_token` is used
/// only when auth is on and `--script-token` wasn't supplied; callers in
/// `main` pass `Uuid::new_v4()`, tests pass a fixed value.
pub fn build_script_config(args: &ScriptCliArgs, fresh_token: Uuid) -> ScriptConfig {
    if !args.script_tcp {
        return ScriptConfig::default();
    }

    let token = if args.script_no_auth {
        None
    } else {
        Some(args.script_token.unwrap_or(fresh_token))
    };

    ScriptConfig {
        tcp: Some(TcpScriptConfig {
            bind: args.script_bind,
            token,
            token_file: args.script_token_file.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_token() -> Uuid {
        // Arbitrary fixed UUID — tests assert it propagates verbatim.
        Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788)
    }

    #[test]
    fn disabled_by_default() {
        let cfg = build_script_config(&ScriptCliArgs::default(), fixed_token());
        assert!(cfg.tcp.is_none());
    }

    #[test]
    fn script_tcp_enables_with_token() {
        let args = ScriptCliArgs {
            script_tcp: true,
            ..Default::default()
        };
        let cfg = build_script_config(&args, fixed_token());
        let tcp = cfg.tcp.expect("tcp should be Some");
        assert_eq!(tcp.bind, default_bind());
        assert_eq!(tcp.token, Some(fixed_token()));
        assert!(tcp.token_file.is_none());
    }

    #[test]
    fn script_token_overrides_fresh_token() {
        let explicit = Uuid::from_u128(0xdead_beef_dead_beef_dead_beef_dead_beef);
        let args = ScriptCliArgs {
            script_tcp: true,
            script_token: Some(explicit),
            ..Default::default()
        };
        let cfg = build_script_config(&args, fixed_token());
        assert_eq!(cfg.tcp.unwrap().token, Some(explicit));
    }

    #[test]
    fn script_no_auth_beats_explicit_token() {
        // clap's conflicts_with enforces this at parse time, but the
        // builder's fallback logic should also produce `None`
        // regardless of script_token when no_auth is set.
        let args = ScriptCliArgs {
            script_tcp: true,
            script_no_auth: true,
            script_token: Some(Uuid::new_v4()),
            ..Default::default()
        };
        assert_eq!(
            build_script_config(&args, fixed_token()).tcp.unwrap().token,
            None
        );
    }

    #[test]
    fn script_no_auth_suppresses_token() {
        let args = ScriptCliArgs {
            script_tcp: true,
            script_no_auth: true,
            ..Default::default()
        };
        let cfg = build_script_config(&args, fixed_token());
        assert_eq!(cfg.tcp.unwrap().token, None);
    }

    #[test]
    fn bind_propagates() {
        let args = ScriptCliArgs {
            script_tcp: true,
            script_bind: "0.0.0.0:45678".parse().unwrap(),
            ..Default::default()
        };
        assert_eq!(
            build_script_config(&args, fixed_token()).tcp.unwrap().bind,
            "0.0.0.0:45678".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn token_file_propagates() {
        let args = ScriptCliArgs {
            script_tcp: true,
            script_token_file: Some(PathBuf::from("/tmp/x.json")),
            ..Default::default()
        };
        assert_eq!(
            build_script_config(&args, fixed_token())
                .tcp
                .unwrap()
                .token_file,
            Some(PathBuf::from("/tmp/x.json"))
        );
    }

    #[test]
    fn flags_requiring_script_tcp_are_ignored_without_it() {
        let args = ScriptCliArgs {
            script_tcp: false,
            script_bind: "0.0.0.0:999".parse().unwrap(),
            script_no_auth: true,
            script_token: None,
            script_token_file: Some(PathBuf::from("/x")),
        };
        assert!(build_script_config(&args, fixed_token()).tcp.is_none());
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
        assert!(parse_bind_spec("127.0.0.1:").is_err()); // trailing colon, no port
    }
}
