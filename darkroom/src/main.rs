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
        .with_setup(|ui, app, handle| {
            app.host_handle = Some(handle.clone());
            // Push the darkroom-tuned palantir theme onto `Ui` once,
            // before the first frame, so every palantir widget reads
            // the same palette as darkroom's own widgets.
            ui.theme = app.theme.palantir_theme.clone();
        })
        .run();
}
