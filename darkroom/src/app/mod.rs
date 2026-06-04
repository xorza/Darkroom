use std::path::PathBuf;
use std::sync::Arc;

use lens::ImageFuncLib;
use palantir::{HostHandle, Ui};
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::WorkerEventsFuncLib;
use scenarium::prelude::{FuncLib, Graph as CoreGraph};

use crate::document::Document;
use crate::io::config::AppConfig;
use crate::io::library;
use crate::run_state::RunState;
use crate::script::{ScriptConfig, ScriptHost, SessionInbound};
use crate::theme::{Theme, ThemeChoice};
use crate::wake::Wake;

mod commands;
pub(crate) mod editor;
pub(crate) mod worker;

use editor::Editor;
use worker::{WorkerBridge, WorkerEvent};

/// The built-in runtime function library. Builtins carry no subgraph
/// defs, so `func_lib.subgraphs` *is* the shared subgraph library —
/// loaded from the library file at startup, grown by "promote".
pub(crate) fn builtin_func_lib() -> FuncLib {
    let mut func_lib = FuncLib::default();
    func_lib.merge(BasicFuncLib::default());
    func_lib.merge(WorkerEventsFuncLib::default());
    func_lib.merge(ImageFuncLib::default());
    func_lib
}

/// Shared per-frame context threaded down the UI tree. Holds borrows
/// of state owned higher up so child subtrees don't take a growing
/// fan-out of `&` parameters.
#[derive(Copy, Clone, Debug)]
pub(crate) struct AppContext<'a> {
    pub(crate) theme: &'a Theme,
    /// The user's persisted theme preference (`system`/`dark`/`light`),
    /// so the Theme menu can mark the active choice. Distinct from
    /// `theme`, the concrete palette `System` resolved to.
    pub(crate) theme_choice: ThemeChoice,
    pub(crate) func_lib: &'a FuncLib,
    /// Last run's per-node state (status, logs, fetched runtime values),
    /// keyed by authoring `NodeId`. Read by the inspection panel's Log and
    /// Inputs/Outputs sections.
    pub(crate) run_state: &'a RunState,
}

/// Thin shell around the [`Editor`] (which owns the document + its edit
/// pipeline + the GUI tree): `App` holds only the runtime/IO the editor
/// borrows each frame — the shared func lib, the active theme, session
/// config + file path, the host handle, and the evaluation worker. Its
/// `frame` drains the worker into the editor's projections, runs one
/// `Editor::frame`, and actions the [`MenuCommand`] it surfaces (file /
/// theme / subgraph dialogs, run) outside the record.
#[derive(Debug)]
pub(crate) struct App {
    pub(crate) editor: Editor,
    /// Shared with the worker on every run (`Arc` so a run clones a
    /// pointer, not the whole lib). Runtime-owned, not part of the
    /// serialized `Document`.
    pub(crate) func_lib: Arc<FuncLib>,
    pub(crate) theme: Theme,
    pub(crate) host_handle: HostHandle,
    /// Last successfully loaded/saved file path. `Save…` and `Load…`
    /// preopen the dialog at this directory so a session that touches
    /// many files in the same folder doesn't re-navigate each time.
    pub(crate) current_path: Option<PathBuf>,
    /// Persisted session state (active theme name + last document).
    /// Written on every doc/theme change so the next launch reopens
    /// where the user left off.
    pub(crate) config: AppConfig,
    /// Drives the headless graph-evaluation worker (run on demand,
    /// results drained each frame). Off the serialized state.
    worker: WorkerBridge,
    /// Scripting-over-TCP host: owns its own tokio runtime + the sandboxed
    /// Rhai executor. `Some` only when `--script-tcp` bound a listener;
    /// `App` drains its inbound queue each frame. Off the serialized state.
    script: Option<ScriptHost>,
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
    pub(crate) fn new(ui: &mut Ui, handle: HostHandle, script_cfg: ScriptConfig) -> Self {
        let mut document: Document = CoreGraph::default().into();
        document.main_view.auto_layout_default(&document.graph);
        // The runtime lib is builtins + the shared subgraph library
        // (where `SubgraphRef::Linked` resolves); builtins carry no
        // subgraphs, so `func_lib.subgraphs` *is* the library.
        let mut func_lib = builtin_func_lib();
        for def in library::load_library() {
            func_lib.add_subgraph(def);
        }
        let func_lib = Arc::new(func_lib);
        // The worker + script host wake the winit loop via the host handle;
        // the headless/tui drivers swap in a tokio `Notify` (see
        // `crate::wake`).
        let wake: Wake = {
            let handle = handle.clone();
            Arc::new(move || handle.request_repaint())
        };
        let worker = WorkerBridge::new(wake.clone());
        // Start the script listener — a no-op returning `None` unless
        // `--script-tcp` was passed. Shares a startup snapshot of the func
        // lib with the executor.
        let script = ScriptHost::start(&script_cfg, func_lib.clone(), wake);
        let mut app = Self {
            editor: Editor::new(document),
            func_lib,
            theme: Theme::default(),
            host_handle: handle,
            current_path: None,
            config: AppConfig::default(),
            worker,
            script,
        };
        app.config = AppConfig::load();
        // Resolve the saved preference: `System` (the default) follows
        // the OS light/dark setting, re-queried each launch.
        app.theme = Theme::from_preset(app.config.theme.resolve());
        if let Some(path) = app.config.document_path.clone() {
            app.load_document(&path);
        }
        // Resolved theme (default, or whatever the config restored)
        // onto the Ui so palantir widgets paint correctly frame 1.
        ui.theme = app.theme.palantir_theme.clone();
        // ui.debug_overlay.damage_rect = true;
        app
    }

    /// Send the whole document graph to the worker and execute its
    /// terminals once. The worker evaluates the full nested graph, so
    /// this is independent of the active tab. Cloning the graph + an
    /// `Arc` bump of the lib is the per-run cost.
    pub(crate) fn run_graph(&mut self) {
        // Open a fresh value-cache epoch: a re-run invalidates last run's
        // per-node values, and tags the replies the open panels request.
        self.editor.run_state.begin_run();
        self.worker
            .run_once(self.editor.document.graph.clone(), self.func_lib.clone());
    }

    /// Consume worker results posted since the last frame. A finished run
    /// reprojects per-node `ExecStatus` (the status glow) and per-node
    /// logs (the inspector's Log section); a failed run clears both. An
    /// argument-value reply lands in the run state (uploading any preview
    /// textures via `ui`). Drained before the editor's scene rebuild so
    /// they reflect the latest run.
    fn drain_worker_events(&mut self, ui: &Ui) {
        let run_state = &mut self.editor.run_state;
        for event in self.worker.drain() {
            match event {
                WorkerEvent::ExecutionFinished(Ok(stats)) => run_state.set_results(&stats),
                WorkerEvent::ExecutionFinished(Err(err)) => {
                    eprintln!("compute failed: {err}");
                    run_state.clear();
                }
                WorkerEvent::ArgumentValues { request, values } => {
                    run_state.ingest_values(ui, request, values)
                }
            }
        }
    }

    /// Forward the editor's pending value requests (open panels with no
    /// value yet) to the worker. Run after the frame's record, when the
    /// panel set is settled; the reply arrives on a later frame's drain.
    fn request_open_panel_values(&mut self) {
        for req in self.editor.take_value_requests() {
            self.worker.request_argument_values(req);
        }
    }

    /// Drain the script executor's inbound queue and act on each message:
    /// graph edits go through the editor's external-intent path (one batch
    /// = one undo entry), `run()` kicks one evaluation, `shutdown()` quits.
    /// Runs before the editor's frame so applied edits show the same frame.
    fn handle_script_inbound(&mut self) {
        let events = match &mut self.script {
            Some(script) => script.drain(),
            None => return,
        };
        let mut run = false;
        for event in events {
            match event {
                SessionInbound::Print { msg } => eprintln!("script: {msg}"),
                SessionInbound::Apply(intents) => self.editor.apply_external_intents(intents),
                SessionInbound::RunOnce => run = true,
                // Shutdown is terminal: quit and drop the rest of the batch
                // (the app is closing, so any remaining edits/runs are moot).
                SessionInbound::Shutdown => {
                    self.host_handle.quit();
                    return;
                }
            }
        }
        // Coalesce: many `run()`s in one drain still kick a single run.
        if run {
            self.run_graph();
        }
    }
}

impl palantir::App for App {
    fn frame(&mut self, ui: &mut Ui) {
        // Drain anything the worker posted since last frame, before the
        // editor rebuilds its scene so the status/log projections it
        // reads reflect the latest run.
        self.drain_worker_events(ui);

        // Apply anything scripts pushed since the last frame (graph edits,
        // run, quit) before the editor rebuilds, so the scene reflects them.
        self.handle_script_inbound();

        let command = self.editor.frame(
            ui,
            &self.func_lib,
            &self.theme,
            self.config.theme,
            &self.host_handle,
        );

        // The frame settled which inspector panels are open; request the
        // runtime values for any that still need them.
        self.request_open_panel_values();

        // Menu side effects run last so the blocking file dialog opens
        // after the frame's record + drain. Loading replaces the
        // document/theme wholesale, so always relayout afterward.
        if let Some(command) = command {
            self.handle_menu_command(ui, command);
            ui.request_relayout();
        }
    }
}
