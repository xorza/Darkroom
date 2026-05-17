#[allow(dead_code)]
mod action_stack;
mod app;
mod document;
#[allow(dead_code)]
mod frame_result;
mod gui;
#[allow(dead_code)]
mod intent;
mod model;
mod scene;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::new(WinitHostConfig::new("darkroom"), App::new()).run();
}
