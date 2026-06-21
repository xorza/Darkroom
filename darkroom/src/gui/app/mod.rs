use std::path::PathBuf;
use std::sync::Arc;

use palantir::Ui;
use scenarium::prelude::{FuncLib, Graph as CoreGraph};

use crate::core::document::Document;
use crate::core::engine::Engine;
use crate::core::io::config::AppConfig;
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::theme_pref::ThemeChoice;
use crate::core::wake::Wake;
use crate::core::worker::WorkerEvent;
use crate::gui::HostHandle;
use crate::gui::MAIN_WINDOW;
use crate::gui::run_state::RunState;
use crate::gui::theme::Theme;

mod commands;
pub(crate) mod editor;

use editor::Editor;

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
/// borrows each frame — the [`Engine`] (func lib + worker + script host),
/// the active theme, session config + file path, and the host handle. Its
/// `frame` drains the engine's worker + script queues into the editor's
/// projections, runs one `Editor::frame`, and actions the [`MenuCommand`]
/// it surfaces (file / theme / subgraph dialogs, run) outside the record.
#[derive(Debug)]
pub(crate) struct App {
    pub(crate) editor: Editor,
    /// Shared runtime services — func lib + evaluation worker + script host
    /// — built at startup. The GUI loads an `engine.func_lib` snapshot each
    /// frame and drains the worker/script queues through it. Off the
    /// serialized state.
    pub(crate) engine: Engine,
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
        // The worker + script host wake the winit loop via the host handle;
        // the headless/tui drivers swap in a tokio `Notify` (see
        // `crate::core::wake`).
        let wake: Wake = {
            let handle = handle.clone();
            Arc::new(move || handle.request_repaint(MAIN_WINDOW))
        };
        let mut app = Self {
            editor: Editor::new(document),
            engine: Engine::new(&script_cfg, wake),
            theme: Theme::default(),
            host_handle: handle,
            current_path: None,
            config: AppConfig::default(),
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
        self.engine.run_once(self.editor.document.graph.clone());
    }

    /// Consume worker results posted since the last frame. A finished run
    /// reprojects per-node `ExecStatus` (the status glow) and per-node
    /// logs (the inspector's Log section); a failed run clears both. An
    /// argument-value reply lands in the run state (uploading any preview
    /// textures via `ui`). Drained before the editor's scene rebuild so
    /// they reflect the latest run.
    fn drain_worker_events(&mut self, ui: &Ui) {
        let run_state = &mut self.editor.run_state;
        for event in self.engine.drain_worker() {
            match event {
                WorkerEvent::ExecutionFinished(Ok(stats)) => run_state.set_results(&stats),
                WorkerEvent::ExecutionFinished(Err(err)) => {
                    eprintln!("compute failed: {err}");
                    run_state.clear();
                }
                WorkerEvent::NodeProgress(progress) => run_state.apply_progress(&progress),
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
            self.engine.request_argument_values(req);
        }
    }

    /// Drain the script executor's inbound queue and act on each message:
    /// graph edits go through the editor's external-intent path (one batch
    /// = one undo entry), `run()` kicks one evaluation, `shutdown()` quits.
    /// Runs before the editor's frame so applied edits show the same frame.
    fn handle_script_inbound(&mut self) {
        let events = self.engine.drain_script();
        let mut run = false;
        for event in events {
            match event {
                ScriptMessage::Print { msg } => eprintln!("script: {msg}"),
                ScriptMessage::Apply(intents) => self.editor.apply_external_intents(intents),
                ScriptMessage::RunOnce => run = true,
                // Shutdown is terminal: quit and drop the rest of the batch
                // (the app is closing, so any remaining edits/runs are moot).
                ScriptMessage::Shutdown => {
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
    fn frame(&mut self, _win: palantir::WindowToken, ui: &mut Ui) {
        // Drain anything the worker posted since last frame, before the
        // editor rebuilds its scene so the status/log projections it
        // reads reflect the latest run.
        self.drain_worker_events(ui);

        // Apply anything scripts pushed since the last frame (graph edits,
        // run, quit) before the editor rebuilds, so the scene reflects them.
        self.handle_script_inbound();

        // One consistent library snapshot for the whole frame (cheap atomic
        // load); a mid-frame promote/publish swap takes effect next frame.
        let func_lib = self.engine.func_lib.load();
        let command = self.editor.frame(
            ui,
            &func_lib,
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
