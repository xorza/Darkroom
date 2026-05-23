#[allow(dead_code)]
mod action_stack;
mod app;
mod document;
mod gui;
#[allow(dead_code)]
mod intent;
mod model;
mod scene;
mod theme;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::new(WinitHostConfig::new("darkroom"), App::new())
        .with_setup(|_ui, app, handle| {
            app.host_handle = Some(handle.clone());
        })
        .run();
}
