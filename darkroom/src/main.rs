mod gui;
mod model;
mod view;

use glam::Vec2;
use palantir::{WinitHost, WinitHostConfig};
use scenarium::prelude::FuncLib;
use scenarium::testing::{TestFuncHooks, test_func_lib, test_graph};

use crate::model::ViewGraph;

pub struct AppState {
    pub view_graph: ViewGraph,
    pub func_lib: FuncLib,
}

impl AppState {
    fn new() -> Self {
        let mut view_graph: ViewGraph = test_graph().into();
        view_graph.auto_layout(220.0, 110.0, Vec2::new(40.0, 40.0));
        let func_lib = test_func_lib(TestFuncHooks::default());
        Self {
            view_graph,
            func_lib,
        }
    }
}

fn main() {
    WinitHost::new(
        WinitHostConfig::new("darkroom"),
        AppState::new(),
        |ui, app| view::build(ui, app),
    )
    .run();
}
