mod app;
mod frame_cache;
mod gui;
mod model;
mod scene;
mod view;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::new(WinitHostConfig::new("darkroom"), App::new()).run();
}
