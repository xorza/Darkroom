//! Terminal frontend (stub). Selected with `--tui`; bypasses
//! eframe entirely and drives [`Session`] through a [`TuiUiHost`]
//! that drops non-terminal signals on the floor.
//!
//! [`Session`]: crate::session::Session

use anyhow::Result;

use crate::tui::app::TuiApp;

pub mod app;
pub mod main_tui;
pub mod ui_host;

pub fn run() -> Result<()> {
    let mut app = TuiApp::new();
    app.run()
}
