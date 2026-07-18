//! Tiny CLI for the darkroom script TCP transport — a hand client for the
//! length-prefixed, lz4-framed protocol, handy when you'd rather not write
//! one just to poke a running darkroom instance.
//!
//! Usage:
//!
//!     # one-shot, exit on reply
//!     cargo run -p darkroom --example script_client -- --exec '40 + 2'
//!
//!     # read script from stdin
//!     echo 'list_funcs().len()' | cargo run -p darkroom --example script_client
//!
//!     # interactive session: each line is one request frame, reply printed
//!     cargo run -p darkroom --example script_client -- --repl
//!
//!     # discover addr+token from the file darkroom wrote with --script-token-file
//!     cargo run -p darkroom --example script_client -- --token-file /tmp/darkroom.json --repl
//!
//! Bring your own running darkroom, e.g.:
//!     cargo run -p darkroom -- --script-tcp --script-bind :34567 --script-token <uuid>

use std::io::{BufRead, Error, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(about = "Tiny TCP client for the darkroom script transport")]
struct Args {
    /// `host:port` of the darkroom listener. Ignored if `--token-file` is set.
    #[arg(long, default_value = "127.0.0.1:34567")]
    addr: String,

    /// 16-byte auth UUID. Required if the server was started with `--script-token`.
    #[arg(long)]
    token: Option<Uuid>,

    /// Path to the JSON discovery file written by `darkroom --script-token-file`.
    /// Overrides `--addr` and `--token`.
    #[arg(long)]
    token_file: Option<PathBuf>,

    /// Resume an existing session by id. Default: ask for a fresh one.
    #[arg(long)]
    session: Option<Uuid>,

    /// One-shot script source. Mutually exclusive with `--repl`.
    /// If neither is set, the script is read from stdin until EOF.
    #[arg(long, short = 'e')]
    exec: Option<String>,

    /// Interactive line-buffered loop: every input line is one request.
    /// Empty lines are skipped; EOF (Ctrl-D) exits.
    #[arg(long, conflicts_with = "exec")]
    repl: bool,
}

#[derive(Deserialize)]
struct Discovery {
    port: u16,
    token: Option<String>,
}

#[derive(Deserialize)]
struct ScriptReply {
    session: Option<String>,
    print: String,
    result: serde_json::Value,
    error: Option<String>,
}

fn load_discovery(path: &Path) -> Result<(String, Option<Uuid>), String> {
    let body = std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
    let d: Discovery = serde_json::from_str(&body).map_err(|e| format!("parse {path:?}: {e}"))?;
    let token = d
        .token
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|e| format!("token in {path:?}: {e}"))?;
    Ok((format!("127.0.0.1:{}", d.port), token))
}

fn send_request(
    stream: &mut TcpStream,
    session: Option<Uuid>,
    source: &[u8],
) -> std::io::Result<()> {
    let id = session.map(|u| u.as_u128()).unwrap_or(0);
    stream.write_all(&id.to_be_bytes())?;
    let body = lz4_flex::block::compress_prepend_size(source);
    let len = u32::try_from(body.len()).expect("script source fits in u32");
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&body)?;
    Ok(())
}

fn read_reply(stream: &mut TcpStream) -> std::io::Result<ScriptReply> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body)?;
    let raw = lz4_flex::block::decompress_size_prepended(&body)
        .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?;
    serde_json::from_slice(&raw).map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))
}

fn render_reply(reply: &ScriptReply) {
    if !reply.print.is_empty() {
        print!("{}", reply.print);
    }
    if let Some(err) = &reply.error {
        eprintln!("error: {err}");
    } else {
        println!("{}", reply.result);
    }
    let _ = std::io::stdout().flush();
}

fn run() -> Result<(), String> {
    let args = Args::parse();

    let (addr, token) = if let Some(p) = args.token_file.as_deref() {
        load_discovery(p)?
    } else {
        (args.addr.clone(), args.token)
    };

    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    if let Some(t) = token {
        stream
            .write_all(&t.as_u128().to_be_bytes())
            .map_err(|e| format!("auth write: {e}"))?;
    }

    let mut session = args.session;
    let mut had_error = false;

    if args.repl {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            print!("> ");
            std::io::stdout().flush().ok();
            line.clear();
            let n = stdin
                .lock()
                .read_line(&mut line)
                .map_err(|e| e.to_string())?;
            if n == 0 {
                println!();
                break;
            }
            let src = line.trim_end_matches(['\n', '\r']);
            if src.is_empty() {
                continue;
            }
            send_request(&mut stream, session, src.as_bytes()).map_err(|e| e.to_string())?;
            let reply = read_reply(&mut stream).map_err(|e| e.to_string())?;
            if session.is_none() {
                session = reply
                    .session
                    .as_deref()
                    .and_then(|s| Uuid::parse_str(s).ok());
            }
            render_reply(&reply);
            if reply.error.is_some() {
                had_error = true;
            }
        }
    } else {
        let source = if let Some(c) = args.exec {
            c
        } else {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| e.to_string())?;
            s
        };
        send_request(&mut stream, session, source.as_bytes()).map_err(|e| e.to_string())?;
        let reply = read_reply(&mut stream).map_err(|e| e.to_string())?;
        render_reply(&reply);
        had_error = reply.error.is_some();
    }

    if had_error {
        Err("script error".into())
    } else {
        Ok(())
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("script_client: {e}");
            ExitCode::FAILURE
        }
    }
}
