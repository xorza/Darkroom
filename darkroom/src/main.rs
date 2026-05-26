mod app;
mod document;
mod edit;
mod exec_status;
mod gui;
mod io;
mod scene;
mod theme;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::new(WinitHostConfig::new("darkroom"), App::new).run();
}
