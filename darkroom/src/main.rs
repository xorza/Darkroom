#[allow(dead_code)]
mod action_stack;
mod app;
mod frame_cache;
#[allow(dead_code)]
mod frame_result;
mod gui;
#[allow(dead_code)]
mod intent;
mod model;
mod scene;
mod view;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::new(WinitHostConfig::new("darkroom"), App::new()).run();
}
