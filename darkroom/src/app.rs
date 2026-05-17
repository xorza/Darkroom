use glam::Vec2;
use palantir::Ui;
use scenarium::testing::{TestFuncHooks, test_func_lib, test_graph};

use crate::action_stack::ActionStack;
use crate::document::Document;
use crate::frame_result::FrameResult;
use crate::gui::main_window::MainWindow;
use crate::intent::{apply_step, build_step, requires_relayout};
use crate::model::ViewGraph;
use crate::scene::Scene;

const UNDO_HISTORY: usize = 100;

#[derive(Debug)]
pub struct App {
    pub document: Document,
    pub scene: Scene,
    pub main_window: MainWindow,
    pub frame_result: FrameResult,
    pub action_stack: ActionStack,
}

impl App {
    pub fn new() -> Self {
        let mut view_graph: ViewGraph = test_graph().into();
        view_graph.auto_layout(220.0, 110.0, Vec2::new(40.0, 40.0));
        let func_lib = test_func_lib(TestFuncHooks::default());
        Self {
            document: Document::new(view_graph, func_lib),
            scene: Scene::default(),
            main_window: MainWindow::default(),
            frame_result: FrameResult::default(),
            action_stack: ActionStack::new(UNDO_HISTORY),
        }
    }
}

impl palantir::App for App {
    fn frame(&mut self, ui: &mut Ui) {
        self.frame_result.clear();
        self.scene.rebuild(&self.document);
        self.main_window
            .frame(ui, &self.scene, &mut self.frame_result);
        let mut relayout = false;
        for intent in self.frame_result.drain() {
            if intent.is_noop_against(&self.document) {
                continue;
            }
            let step = build_step(intent, &self.document);
            apply_step(&step, &mut self.document);
            relayout |= requires_relayout(&step);
            self.action_stack.push_current(std::slice::from_ref(&step));
        }
        if relayout {
            ui.request_relayout();
        }
    }
}
