//! Terminal frontend (stub). Selected by passing `tui` on the
//! command line; bypasses eframe entirely and drives [`Session`]
//! through a [`TuiUiHost`] that drops non-terminal signals on the
//! floor.

use std::io::{BufRead, Write};

use anyhow::Result;

use crate::session::Session;
use crate::ui_host::UiHost;

#[derive(Debug)]
pub struct TuiUiHost;

impl TuiUiHost {
    pub fn new() -> Self {
        Self
    }
}

impl UiHost for TuiUiHost {
    fn request_redraw(&self) {}
    fn close_app(&self) {}
}

#[derive(Debug)]
pub struct MainTui;

impl MainTui {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&mut self, session: &mut Session) -> Result<()> {
        println!("Prism TUI (stub). Type 'help' for commands.");
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut line = String::new();

        loop {
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
