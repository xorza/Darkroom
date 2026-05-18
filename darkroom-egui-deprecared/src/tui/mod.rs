//! Terminal frontend (stub). Selected with `--tui`; bypasses
//! eframe entirely and drives [`Session`] through a [`TuiUiHost`]
//! whose redraw/close hooks wake an async loop that ticks
//! `Session::frame` between stdin reads, so script side-effects
//! (Apply, Print, Run*) drain instead of leaking.
//!
//! [`Session`]: crate::session::Session

use anyhow::Result;

use crate::launch_config::LaunchConfig;
use crate::tui::app::TuiApp;

pub mod app;
pub mod main_tui;
pub mod ui_host;

pub async fn run(launch_config: LaunchConfig) -> Result<()> {
    let mut app = TuiApp::new(launch_config);
    app.run().await
}
