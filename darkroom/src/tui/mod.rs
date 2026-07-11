//! Terminal command shell — a faithful successor to the old TUI stub: a
//! line-buffered REPL over stdin, *not* a graph renderer. It drives a
//! [`Session`] between stdin reads so script side-effects (apply / print /
//! run / shutdown) drain instead of piling up in the inbound channel.
//!
//! Commands: `help`, `status`, `nodes`, `run`, `save`, `quit`. The
//! `Session` is owned by `main` (built + dropped in sync context, since
//! dropping the worker/script tokio runtimes inside this async loop would
//! panic); here we only borrow it.

use std::io::Write;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;

use crate::core::session::Session;

pub(crate) async fn run(session: &mut Session, notify: &Notify) -> Result<()> {
    println!("darkroom TUI — type 'help' for commands.");
    prompt()?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    loop {
        session.tick();
        if session.should_quit() {
            break;
        }
        tokio::select! {
            // Woken by a script side-effect: loop back to drain (and print
            // any worker results), without re-prompting.
            _ = notify.notified() => continue,
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                if handle_command(session, line.trim()) {
                    break;
                }
                prompt()?;
            }
        }
    }
    Ok(())
}

/// Run one shell command. Returns `true` when the shell should exit.
fn handle_command(session: &mut Session, cmd: &str) -> bool {
    match cmd {
        "" => {}
        "help" => println!("commands: help, status, nodes, run, save, quit"),
        "status" => {
            let mut any = false;
            for line in session.engine.status.lines() {
                println!("{line}");
                any = true;
            }
            if !any {
                println!("(empty)");
            }
        }
        "nodes" => println!("{} node(s)", session.node_count()),
        "run" => {
            session.run_graph();
            println!("run queued");
        }
        "save" => {
            if session.save() {
                println!("saved");
            } else {
                println!("nothing to save (no document was opened)");
            }
        }
        "quit" | "exit" => return true,
        other => println!("unknown command: {other} (try 'help')"),
    }
    false
}

fn prompt() -> Result<()> {
    print!("> ");
    std::io::stdout().flush()?;
    Ok(())
}
