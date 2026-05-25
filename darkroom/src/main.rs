#[allow(dead_code)]
mod action_stack;
mod app;
mod config;
mod document;
mod gui;
#[allow(dead_code)]
mod intent;
mod model;
mod persistence;
mod sample_graph;
mod scene;
mod theme;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::<App>::new(WinitHostConfig::new("darkroom")).run();
}
