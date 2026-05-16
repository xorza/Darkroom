mod model;
mod view;

use glam::Vec2;
use palantir::{WinitHost, WinitHostConfig};
use scenarium::testing::test_graph;

use crate::model::ViewGraph;

struct AppState {
    view_graph: ViewGraph,
}

impl AppState {
    fn new() -> Self {
        let mut view_graph: ViewGraph = test_graph().into();
        view_graph.auto_layout(220.0, 110.0, Vec2::new(40.0, 40.0));
        Self { view_graph }
    }
}

fn main() {
    WinitHost::new(
        WinitHostConfig::new("darkroom"),
        AppState::new(),
        |ui, app| view::build(ui, &app.view_graph),
    )
    .run();
}
