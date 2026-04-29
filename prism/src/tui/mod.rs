//! Terminal frontend (stub). Selected with `--tui`; bypasses
//! eframe entirely and drives [`Session`] through a [`TuiUiHost`]
//! that drops non-terminal signals on the floor.
//!
//! [`Session`]: crate::session::Session

use anyhow::Result;

use crate::launch_config::LaunchConfig;
use crate::tui::app::TuiApp;

pub mod app;
pub mod main_tui;
pub mod ui_host;

pub fn run(launch_config: LaunchConfig) -> Result<()> {
    let mut app = TuiApp::new(launch_config);
    app.run()
}
