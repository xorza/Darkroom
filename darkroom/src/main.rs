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
mod scene;
mod theme;

use palantir::{WinitHost, WinitHostConfig};

use crate::app::App;

fn main() {
    WinitHost::new(WinitHostConfig::new("darkroom"), App::new())
        .with_setup(|ui, app, handle| {
            app.host_handle = Some(handle.clone());
            // Restore persisted config (theme + last document) and
            // push the resolved palantir theme onto `Ui` before the
            // first frame.
            app.startup(ui);
        })
        .run();
}
