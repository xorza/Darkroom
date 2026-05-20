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
use crate::theme::Theme;

const UNDO_HISTORY: usize = 100;

/// Shared per-frame context threaded down the UI tree. Holds borrows
/// of state owned higher up so child subtrees don't take a growing
/// fan-out of `&` parameters. Currently just the active [`Theme`];
/// future per-frame shared state (selection, debug toggles, etc.)
/// lives here too.
#[derive(Copy, Clone, Debug)]
pub struct AppContext<'a> {
    pub theme: &'a Theme,
}

impl<'a> AppContext<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

#[derive(Debug)]
pub struct App {
    pub document: Document,
    pub scene: Scene,
    pub main_window: MainWindow,
    pub frame_result: FrameResult,
    pub action_stack: ActionStack,
    pub theme: Theme,
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
            theme: Theme::default(),
        }
    }
}

impl palantir::App for App {
    fn frame(&mut self, ui: &mut Ui) {
        ui.debug_overlay.damage_rect = true;

        // Prepass: each UI subtree pushes input-derived intents
        // (drag-driven `MoveNode`, etc.) into `frame_result`. Drained
        // and applied *before* `Scene::rebuild`, so Pass A's record
        // sees the freshly-mutated doc — no Pass B retry for drag.
        self.frame_result.clear();
        self.main_window
            .prepass(ui, &self.scene, &mut self.frame_result);
        let relayout = self.drain_intents();

        // Record. Widgets push intents derived from record-time state
        // (button clicks, edit commits) into `frame_result`.
        self.scene.rebuild(&self.document);
        let ctx = AppContext::new(&self.theme);
        self.main_window
            .frame(ui, &ctx, &mut self.scene, &mut self.frame_result);

        // Post-record drain — these intents reflect mutations that
        // only the now-just-completed record could surface, so they
        // *do* warrant a relayout retry when applicable.
        if self.drain_intents() | relayout {
            ui.request_relayout();
        }
    }
}

impl App {
    /// Drain `frame_result`, apply each non-noop intent to `document`,
    /// and push the resulting step onto the undo stack. Returns
    /// whether any applied step needs a relayout retry.
    fn drain_intents(&mut self) -> bool {
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
        relayout
    }
}
