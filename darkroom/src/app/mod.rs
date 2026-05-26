use std::path::PathBuf;

use lens::ImageFuncLib;
use palantir::{HostHandle, Ui};
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::WorkerEventsFuncLib;
use scenarium::prelude::FuncLib;
use scenarium::testing::{TestFuncHooks, test_func_lib};

use crate::document::sample_graph::sample_graph;
use crate::document::{Document, GraphRef};
use crate::edit::action_stack::ActionStack;
use crate::edit::intent::{Intent, apply_step, build_step, requires_relayout};
use crate::gui::UiAction;
use crate::gui::main_window::MainWindow;
use crate::io::config::AppConfig;
use crate::scene::Scene;
use crate::theme::Theme;

mod commands;
mod shortcuts;

/// Byte budget for the undo history's packed buffer (~1 MiB). Bounds
/// memory rather than entry count — a single large edit can't be
/// undone away, but the oldest entries drop once the buffer overflows.
const UNDO_HISTORY_BYTES: usize = 1 << 20;

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

#[derive(Debug)]
pub struct App {
    pub document: Document,
    pub func_lib: FuncLib,
    pub scene: Scene,
    /// Which graph `scene` last reflected. A mismatch with the active
    /// target means the tab changed: drop transient gesture state
    /// (`reset_transient`) and request a relayout. `None` forces that on
    /// the next frame (set by document replacement). It does *not* gate
    /// the rebuild itself — the projection is rebuilt every frame
    /// regardless (see `frame`).
    scene_target: Option<GraphRef>,
    /// Set by `drain_intents` whenever it applies a step; consumed by the
    /// pre-record rebuild so the record sees doc edits the pre-record
    /// drain made (drag, connection commit). Only meaningful in the
    /// window between the unconditional pre-prepass rebuild (which clears
    /// it) and the pre-record rebuild.
    scene_dirty: bool,
    pub main_window: MainWindow,
    /// Per-frame scratch buffer of pending mutations. Cleared at the
    /// top of every `frame`, filled by prepass/record/shortcut
    /// handling, and fully drained before `frame` returns — it carries
    /// no state across frames. Kept as a field only to reuse the
    /// allocation; not part of `App`'s observable state.
    intents: Vec<Intent>,
    /// Per-frame scratch buffer of view-state requests (open/activate/
    /// close tab) raised during record. Drained each frame; carries no
    /// cross-frame state — kept only to reuse the allocation.
    actions: Vec<UiAction>,
    pub action_stack: ActionStack,
    pub theme: Theme,
    pub host_handle: HostHandle,
    /// Last successfully loaded/saved file path. `Save…` and `Load…`
    /// preopen the dialog at this directory so a session that touches
    /// many files in the same folder doesn't re-navigate each time.
    pub current_path: Option<PathBuf>,
    /// Persisted session state (active theme name + last document).
    /// Written on every doc/theme change so the next launch reopens
    /// where the user left off.
    pub config: AppConfig,
}

impl App {
    /// Build the app before the first frame: assemble the func lib +
    /// seed document, then restore persisted config (saved theme +
    /// last document) and push the resolved palantir theme onto `Ui`.
    /// Restore failures degrade silently to defaults — a missing or
    /// corrupt config, or a deleted document, must not block launch.
    ///
    /// Handed to [`palantir::WinitHost::run`], which calls it once the
    /// `Ui` + [`HostHandle`] exist (before the first frame).
    pub(crate) fn new(ui: &mut Ui, handle: HostHandle) -> Self {
        let mut document: Document = sample_graph().into();
        document.main_view.auto_layout_default(&document.graph);
        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(TestFuncHooks::default()));
        func_lib.merge(BasicFuncLib::default());
        func_lib.merge(WorkerEventsFuncLib::default());
        func_lib.merge(ImageFuncLib::default());
        let mut app = Self {
            document,
            func_lib,
            scene: Scene::default(),
            scene_target: None,
            scene_dirty: false,
            main_window: MainWindow::default(),
            intents: Vec::new(),
            actions: Vec::new(),
            action_stack: ActionStack::new(UNDO_HISTORY_BYTES),
            theme: Theme::default(),
            host_handle: handle,
            current_path: None,
            config: AppConfig::default(),
        };
        app.config = AppConfig::load();
        if let Some(name) = app.config.theme_name.clone() {
            app.load_theme_file(&AppConfig::theme_path(&name));
        }
        if let Some(path) = app.config.document_path.clone() {
            app.load_document(&path);
        }
        // Resolved theme (default, or whatever the config restored)
        // onto the Ui so palantir widgets paint correctly frame 1.
        ui.theme = app.theme.palantir_theme.clone();
        // ui.debug_overlay.damage_rect = true;
        app
    }
}

impl palantir::App for App {
    fn frame(&mut self, ui: &mut Ui) {
        // ui.debug_overlay.damage_rect = true;

        self.intents.clear();
        self.actions.clear();

        // ── Navigation phase ─────────────────────────────────────────
        // Settle the active graph entirely from frame-top inputs
        // (keyboard undo/redo + last-frame click responses). `navigate`
        // reads *last* frame's `scene` to resolve tab/chip clicks, so it
        // must run before this frame's rebuild. After it, `target` is
        // fixed for the rest of the frame.
        let mut relayout = self.navigate(ui);
        let target = self.document.active_target();
        relayout |= self.sync_target(target);

        // Rebuild the projection for this frame, after the navigation
        // phase has fully settled the document — so prepass and
        // `PortFrame` never read a stale graph (the old "rebuild only on
        // tab switch" path let an undo/redo leave them a frame behind).
        // Unconditional: `Scene` re-interns port names into palantir's
        // per-frame text arena, which clears each `Ui::frame`, so the
        // projection must be regenerated every frame regardless.
        self.rebuild_scene(target);
        self.scene_dirty = false;

        // ── Edit phase ───────────────────────────────────────────────
        // Prepass emits input-derived graph mutations (drag, pan/zoom,
        // connection commit) drained *before* the record so Pass A sees
        // the settled doc. It reads everything off `Scene`, so it takes
        // neither the func lib nor the full `AppContext`.
        self.main_window.prepass(ui, &self.scene, &mut self.intents);
        relayout |= self.drain_intents(target);
        relayout |= self.apply_canvas_shortcuts(ui, target);

        let command_from_shortcut = self.menu_shortcut(ui);

        // Record. Rebuild again only if the pre-record drain actually
        // changed the doc (drag, connection commit) — an idle frame or a
        // bare tab switch leaves `scene_dirty` false and skips it, so the
        // tab-switch frame rebuilds once, not twice.
        if self.scene_dirty {
            self.rebuild_scene(target);
            self.scene_dirty = false;
        }
        let ctx = AppContext {
            theme: &self.theme,
            func_lib: &self.func_lib,
        };
        let host = self.host_handle.clone();
        let command = self
            .main_window
            .frame(
                ui,
                &ctx,
                &mut self.scene,
                Some(&host),
                &self.document,
                &mut self.intents,
            )
            .or(command_from_shortcut);

        // Post-record drain — graph edits the record surfaced (node
        // select, cache toggle, const edit). Navigation is fully settled
        // in the navigation phase, so nothing here moves the active tab.
        relayout |= self.drain_intents(target);
        if relayout {
            ui.request_relayout();
        }

        // Menu side effects run last so the blocking file dialog opens
        // after the frame's record + drain. Loading replaces the
        // document/theme wholesale, so always relayout afterward
        // regardless of what `drain_intents` decided.
        if let Some(command) = command {
            self.handle_menu_command(ui, command);
            ui.request_relayout();
        }
    }
}

impl App {
    /// Settle which graph is active for this frame, from inputs all
    /// available before the record: keyboard undo/redo (which can replay
    /// a `SwitchTab`) and tab/subgraph-open clicks read from *last*
    /// frame's responses. Returns whether a relayout is needed.
    ///
    /// Done up front so the edit pipeline runs against a fixed target and
    /// a switched-to graph records in the same present's Pass A.
    fn navigate(&mut self, ui: &mut Ui) -> bool {
        let mut relayout = self.apply_undo_redo(ui);
        // Surface tab/open clicks from last frame's responses. `scene`
        // still holds the last-rendered graph here — exactly the one
        // whose chips were clicked.
        let tab_count = self.document.tabs.len();
        self.main_window
            .scan_navigation(ui, &self.scene, tab_count, &mut self.actions);
        // Open mutates the tab list directly; activate/close queue
        // undoable `SwitchTab` / `CloseTab` intents — drain them (both
        // steps are graph-agnostic, so the target passed here doesn't
        // matter).
        relayout |= self.apply_view_actions();
        relayout |= self.drain_intents(self.document.active_target());
        // A closed/deleted target can't be active; fall back to Main.
        self.document.ensure_valid_active();
        relayout
    }

    /// Note a possible active-graph change: when `target` differs from
    /// what `scene` last reflected, drop transient gesture state (so a
    /// drag started on one graph can't bleed into another) and request a
    /// relayout. Keeps `PortFrame`'s offset cache, so a graph shown again
    /// resolves its port centers immediately. The rebuild itself is the
    /// caller's unconditional one — this only reacts to the switch.
    fn sync_target(&mut self, target: GraphRef) -> bool {
        if self.scene_target == Some(target) {
            return false;
        }
        self.main_window.reset_transient();
        self.scene_target = Some(target);
        true
    }

    /// Rebuild the `Scene` projection from the graph + view the `target`
    /// points at. Centralizes the borrow split (mut `scene`, shared
    /// `document` + `func_lib`) so the frame can rebuild at more than one
    /// point without repeating it. For a `Local` target, also hands the
    /// scene the enclosing `SubgraphDef` so the interior's boundary nodes
    /// can mirror its interface as their ports.
    ///
    /// Reconciles every subgraph's interface against its interior wiring
    /// first (derived state, like the scene itself) so boundary nodes
    /// render the right ports + placeholder and the doc is consistent
    /// before any save.
    fn rebuild_scene(&mut self, target: GraphRef) {
        self.document.reconcile_boundaries(&self.func_lib);
        let graph = self
            .document
            .graph_for(target)
            .expect("active tab graph exists");
        let view = self.document.view(target).expect("active tab view exists");
        let ctx_def = match target {
            GraphRef::Main => None,
            GraphRef::Local(id) => self.document.graph.subgraphs.by_key(&id),
        };
        self.scene.rebuild(graph, view, &self.func_lib, ctx_def);
    }

    /// Drain `intents`, applying each non-no-op intent to `document`,
    /// and push the whole frame's resulting steps onto the undo stack
    /// as a single batch entry — so a gesture that emits N intents
    /// (e.g. breaker swipe deleting K nodes + unbinding M ports) is
    /// one Cmd-Z. Marks the scene dirty when anything applied (so the
    /// pre-record rebuild folds the change in). Returns whether any
    /// applied step needs a relayout retry.
    fn drain_intents(&mut self, target: GraphRef) -> bool {
        let mut relayout = false;
        let mut batch = Vec::new();
        for intent in self.intents.drain(..) {
            let Some(step) = build_step(intent, &self.document, target) else {
                continue;
            };
            if step.is_noop() {
                continue;
            }
            apply_step(&step, &mut self.document, target);
            relayout |= requires_relayout(&step);
            batch.push(step);
        }
        if !batch.is_empty() {
            self.scene_dirty = true;
            self.action_stack.push_current(target, &batch);
        }
        relayout
    }

    /// Apply the record pass's view-state requests. Open mutates the tab
    /// list directly (not undoable); activate and close are queued as
    /// `Intent::SwitchTab` / `Intent::CloseTab` so they join the undo
    /// history. Returns whether a relayout is needed (the queued intents'
    /// relayout is decided later, when they drain).
    fn apply_view_actions(&mut self) -> bool {
        let mut relayout = false;
        for action in std::mem::take(&mut self.actions) {
            match action {
                UiAction::OpenGraph(target) => relayout |= self.open_graph(target),
                UiAction::ActivateTab(index) => {
                    self.intents.push(Intent::SwitchTab { to: index });
                }
                UiAction::CloseTab(index) => {
                    self.intents.push(Intent::CloseTab { index });
                }
                UiAction::NewSubgraph => {
                    // Create the def then open it; like `OpenGraph`, this
                    // isn't undoable (the new def is referenced by no undo
                    // history, so the stack stays valid).
                    let id = self.document.create_subgraph();
                    relayout |= self.open_graph(GraphRef::Local(id));
                }
            }
        }
        relayout
    }

    /// Focus `target`'s tab, opening a new one (lazily seeding its view
    /// metadata) if not already open. Returns whether anything changed.
    fn open_graph(&mut self, target: GraphRef) -> bool {
        if let Some(index) = self.document.tabs.iter().position(|t| *t == target) {
            if self.document.active != index {
                self.document.active = index;
                return true;
            }
            return false;
        }
        if let GraphRef::Local(id) = target
            && !self.document.ensure_sub_view(id)
        {
            return false;
        }
        self.document.tabs.push(target);
        self.document.active = self.document.tabs.len() - 1;
        true
    }
}
