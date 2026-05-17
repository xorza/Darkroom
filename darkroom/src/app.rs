use glam::Vec2;
use palantir::Ui;
use scenarium::prelude::FuncLib;
use scenarium::testing::{TestFuncHooks, test_func_lib, test_graph};

use crate::frame_cache::FrameCache;
use crate::model::ViewGraph;
use crate::scene::Scene;
use crate::view;

pub struct App {
    pub view_graph: ViewGraph,
    pub func_lib: FuncLib,
    pub scene: Scene,
    pub frame_cache: FrameCache,
}

impl App {
    pub fn new() -> Self {
        let mut view_graph: ViewGraph = test_graph().into();
        view_graph.auto_layout(220.0, 110.0, Vec2::new(40.0, 40.0));
        let func_lib = test_func_lib(TestFuncHooks::default());
        Self {
            view_graph,
            func_lib,
            scene: Scene::default(),
            frame_cache: FrameCache::default(),
        }
    }
}

impl palantir::App for App {
    fn frame(&mut self, ui: &mut Ui) {
        self.scene.rebuild(&self.view_graph, &self.func_lib, ui);
        view::build(ui, &self.scene, &mut self.frame_cache);
    }
}
