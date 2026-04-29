use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use crate::session::Session;

#[derive(Debug)]
pub struct MainTui;

impl MainTui {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&mut self, session: &mut Session, shutdown: &Arc<AtomicBool>) -> Result<()> {
        println!("Prism TUI (stub). Type 'help' for commands.");
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut line = String::new();

        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            print!("> ");
            stdout.flush()?;

            line.clear();
            if stdin.lock().read_line(&mut line)? == 0 {
                break;
            }
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            match line.trim() {
                "" => continue,
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
        }
        Ok(())
    }
}
