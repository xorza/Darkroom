//! Runtime configuration assembled from CLI flags and handed to the
//! chosen frontend. A single aggregate keeps frontend constructors
//! future-proof: new runtime knobs land as fields here rather than as
//! additional arguments.

use std::path::PathBuf;

use clap::Args;
use uuid::Uuid;

use crate::script::{ScriptConfig, TcpScriptConfig};

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub script: ScriptConfig,
}

/// Raw CLI flags for the scripting surface, flattened into the top-level
/// `Cli` with `#[command(flatten)]`. Kept as its own struct so
/// [`build_script_config`] can be unit-tested without going through clap.
#[derive(Debug, Default, Args)]
pub struct ScriptCliArgs {
    /// Enable the loopback TCP script listener (off by default).
    #[arg(long, global = true)]
    pub script_tcp: bool,

    /// Bind port for the TCP script listener. `0` lets the OS pick.
    #[arg(
        long,
        value_name = "PORT",
        default_value_t = 0,
        global = true,
        requires = "script_tcp"
    )]
    pub script_port: u16,

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

    /// Skip token authentication on the TCP script listener. Loopback
    /// only — any local process can connect. Not recommended.
    #[arg(long, global = true, requires = "script_tcp")]
    pub script_no_auth: bool,
}

/// Build a [`ScriptConfig`] from parsed CLI flags. `fresh_token` is the
/// token to embed when auth is enabled; the caller passes a fresh
/// `Uuid::new_v4()` from `main` (or a fixed Uuid in tests).
pub fn build_script_config(args: &ScriptCliArgs, fresh_token: Uuid) -> ScriptConfig {
    if !args.script_tcp {
        return ScriptConfig::default();
    }

    let token = if args.script_no_auth {
        None
    } else {
        Some(fresh_token)
    };

    ScriptConfig {
        tcp: Some(TcpScriptConfig {
            port: args.script_port,
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
        assert_eq!(tcp.port, 0);
        assert_eq!(tcp.token, Some(fixed_token()));
        assert!(tcp.token_file.is_none());
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
    fn port_propagates() {
        let args = ScriptCliArgs {
            script_tcp: true,
            script_port: 45678,
            ..Default::default()
        };
        assert_eq!(
            build_script_config(&args, fixed_token()).tcp.unwrap().port,
            45678
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
        // Without --script-tcp, port / token-file / no-auth have no effect.
        let args = ScriptCliArgs {
            script_tcp: false,
            script_port: 999,
            script_no_auth: true,
            script_token_file: Some(PathBuf::from("/x")),
        };
        assert!(build_script_config(&args, fixed_token()).tcp.is_none());
    }
}
