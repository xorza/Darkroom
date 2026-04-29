use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;

use crate::session::Session;
use crate::session::output::FrameOutput;

#[derive(Debug)]
pub struct MainTui;

impl MainTui {
    pub fn new() -> Self {
        Self
    }

    pub async fn run(
        &mut self,
        session: &mut Session,
        wake: &Arc<Notify>,
        shutdown: &Arc<AtomicBool>,
    ) -> Result<()> {
        println!("Prism TUI (stub). Type 'help' for commands.");
        prompt()?;

        let mut output = FrameOutput::default();
        let mut lines = BufReader::new(tokio::io::stdin()).lines();

        loop {
            // Drain script inbounds (Apply / Print / Run*) and forward
            // any pending worker messages — script-driven graph mutations
            // would otherwise leak into the unbounded inbound channel
            // until shutdown.
            session.tick(&mut output);

            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            tokio::select! {
                // Woken by a script side-effect (`request_redraw`) or by
                // `close_app`. Either way, loop back to drain — the
                // shutdown check at the top handles the close case.
                _ = wake.notified() => continue,
                line = lines.next_line() => {
                    let Some(line) = line? else { break };
                    match line.trim() {
                        "" => {}
                        "help" => println!("commands: help, status, quit"),
                        "status" => {
                            let status = session.status();
                            if status.is_empty() {
                                println!("(empty)");
                            } else {
                                println!("{status}");
                            }
                        }
                        "quit" | "exit" => break,
                        other => println!("unknown command: {other}"),
                    }
                    prompt()?;
                }
            }
        }
        Ok(())
    }
}

fn prompt() -> Result<()> {
    print!("> ");
    std::io::stdout().flush()?;
    Ok(())
}
