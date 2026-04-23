//! Terminal frontend (stub). Selected by passing `tui` on the
//! command line; bypasses eframe entirely and drives [`Session`]
//! through a [`TuiUiHost`] that signals shutdown via an
//! `AtomicBool` the command loop polls.

use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use crate::session::Session;
use crate::ui_host::UiHost;

#[derive(Debug)]
pub struct TuiUiHost {
    should_close: Arc<AtomicBool>,
}

impl TuiUiHost {
    pub fn new() -> Self {
        Self {
            should_close: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn should_close(&self) -> Arc<AtomicBool> {
        self.should_close.clone()
    }
}

impl UiHost for TuiUiHost {
    fn request_redraw(&self) {}

    fn close_app(&self) {
        self.should_close.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug)]
pub struct MainTui;

impl MainTui {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&mut self, session: &mut Session, should_close: &AtomicBool) -> Result<()> {
        println!("Prism TUI (stub). Type 'help' for commands.");
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut line = String::new();

        loop {
            if should_close.load(Ordering::Relaxed) {
                break;
            }
            print!("> ");
            stdout.flush()?;

            line.clear();
            if stdin.lock().read_line(&mut line)? == 0 {
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
