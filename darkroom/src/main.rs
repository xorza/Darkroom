mod action_stack;
mod app;
mod config;
mod document;
mod gui;
mod intent;
mod model;
mod persistence;
mod reconcile;
mod sample_graph;
mod scene;
mod theme;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::<App>::new(WinitHostConfig::new("darkroom")).run();
}
