use glam::Vec2;
use lens::ImageFuncLib;
use palantir::{HostHandle, Shortcut, Ui};
use scenarium::data::StaticValue;
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::WorkerEventsFuncLib;
use scenarium::graph::Binding;
use scenarium::prelude::{FuncLib, NodeId};
use scenarium::testing::{TestFuncHooks, test_func_lib, test_graph};

use crate::action_stack::ActionStack;
use crate::document::Document;
use crate::gui::main_window::MainWindow;
use crate::intent::{self, Intent, apply_step, build_step, requires_relayout};
use crate::model::ViewGraph;
use crate::scene::Scene;
use crate::theme::Theme;

const UNDO_SHORTCUT: Shortcut = Shortcut::cmd('Z');
const REDO_SHORTCUT: Shortcut = Shortcut::cmd_shift('Z');

const UNDO_HISTORY: usize = 100;

/// Shared per-frame context threaded down the UI tree. Holds borrows
/// of state owned higher up so child subtrees don't take a growing
/// fan-out of `&` parameters. Currently just the active [`Theme`];
/// future per-frame shared state (selection, debug toggles, etc.)
/// lives here too.
#[derive(Copy, Clone, Debug)]
pub struct AppContext<'a> {
    pub theme: &'a Theme,
    pub func_lib: &'a FuncLib,
}

impl<'a> AppContext<'a> {
    pub fn new(theme: &'a Theme, func_lib: &'a FuncLib) -> Self {
        Self { theme, func_lib }
    }
}

#[derive(Debug)]
pub struct App {
    pub document: Document,
    pub scene: Scene,
    pub main_window: MainWindow,
    pub intents: Vec<Intent>,
    pub action_stack: ActionStack,
    pub theme: Theme,
    pub host_handle: Option<HostHandle>,
}

impl App {
    pub fn new() -> Self {
        let mut view_graph: ViewGraph = test_graph().into();
        seed_const_bindings(&mut view_graph);
        view_graph.auto_layout(220.0, 110.0, Vec2::new(40.0, 40.0));
        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(TestFuncHooks::default()));
        func_lib.merge(BasicFuncLib::default());
        func_lib.merge(WorkerEventsFuncLib::default());
        func_lib.merge(ImageFuncLib::default());
        Self {
            document: Document::new(view_graph, func_lib),
            scene: Scene::default(),
            main_window: MainWindow::default(),
            intents: Vec::new(),
            action_stack: ActionStack::new(UNDO_HISTORY),
            theme: Theme::default(),
            host_handle: None,
        }
    }
}

impl palantir::App for App {
    fn frame(&mut self, ui: &mut Ui) {
        ui.debug_overlay.damage_rect = true;

        // Prepass: each UI subtree pushes input-derived intents
        // (drag-driven `MoveNode`, etc.) into `intents`. Drained and
        // applied *before* `Scene::rebuild`, so Pass A's record sees
        // the freshly-mutated doc — no Pass B retry for drag.
        self.intents.clear();
        self.main_window.prepass(ui, &self.scene, &mut self.intents);
        let mut relayout = self.drain_intents();
        relayout |= self.handle_shortcuts(ui);

        // Record. Widgets push intents derived from record-time state
        // (button clicks, edit commits) into `intents`.
        self.scene.rebuild(&self.document);
        let ctx = AppContext::new(&self.theme, &self.document.func_lib);
        let host = self.host_handle.clone();
        self.main_window
            .frame(ui, &ctx, &mut self.scene, host.as_ref(), &mut self.intents);

        // Post-record drain — these intents reflect mutations that
        // only the now-just-completed record could surface, so they
        // *do* warrant a relayout retry when applicable.
        if self.drain_intents() | relayout {
            ui.request_relayout();
        }
    }
}

/// Replace a few of `test_graph`'s `Binding::Bind` inputs with
/// `Binding::Const(..)` so the inline static-value editor has
/// something to render on first launch. Targets the `mult` and `sum`
/// nodes by their well-known ids from `scenarium::testing::test_graph`.
fn seed_const_bindings(view_graph: &mut ViewGraph) {
    let mult_id: NodeId = "579ae1d6-10a3-4906-8948-135cb7d7508b".into();
    let sum_id: NodeId = "999c4d37-e0eb-4856-be3f-ad2090c84d8c".into();
    if let Some(node) = view_graph.graph.by_id_mut(&mult_id) {
        node.inputs[1].binding = Binding::Const(StaticValue::Int(7));
    }
    if let Some(node) = view_graph.graph.by_id_mut(&sum_id) {
        node.inputs[1].binding = Binding::Const(StaticValue::Float(2.5));
    }
}

impl App {
    /// Handle Cmd+Z / Cmd+Shift+Z. Suppressed while a widget holds
    /// keyboard focus so Cmd+Z inside a TextEdit doesn't nuke the graph.
    fn handle_shortcuts(&mut self, ui: &mut Ui) -> bool {
        if ui.focused_id().is_some() {
            return false;
        }
        let mut relayout = false;
        let mut on_step = |step: &intent::UndoStep| {
            relayout |= requires_relayout(step);
        };
        if ui.key_pressed(UNDO_SHORTCUT) {
            self.action_stack.undo(&mut self.document, &mut on_step);
        } else if ui.key_pressed(REDO_SHORTCUT) {
            self.action_stack.redo(&mut self.document, &mut on_step);
        }
        relayout
    }

    /// Drain `intents`, applying each non-no-op intent to `document`,
    /// and push the whole frame's resulting steps onto the undo stack
    /// as a single batch entry — so a gesture that emits N intents
    /// (e.g. breaker swipe deleting K nodes + unbinding M ports) is
    /// one Cmd-Z. Returns whether any applied step needs a relayout
    /// retry.
    fn drain_intents(&mut self) -> bool {
        let mut relayout = false;
        let mut batch = Vec::new();
        for intent in self.intents.drain(..) {
            let Some(step) = build_step(intent, &self.document) else {
                continue;
            };
            if step.is_noop() {
                continue;
            }
            apply_step(&step, &mut self.document);
            relayout |= requires_relayout(&step);
            batch.push(step);
        }
        if !batch.is_empty() {
            self.action_stack.push_current(&batch);
        }
        relayout
    }
}
