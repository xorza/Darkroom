mod app;
mod document;
mod edit;
mod gui;
mod io;
mod node_values;
mod run_state;
mod scene;
mod theme;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::new(WinitHostConfig::new("darkroom"), App::new).run();
}
