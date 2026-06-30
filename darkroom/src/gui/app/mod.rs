use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use palantir::Ui;
use scenarium::data::FsPathConfig;
use scenarium::prelude::{Graph as CoreGraph, Library, NodeId};

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

/// A deferred, side-effecting command a UI surface (the menu bar, the graph
/// toolbar, the Config tab, a node's S-badge, an inline path-picker) hands to
/// [`App`] to perform *outside* the record pass — after the frame's record +
/// drain, so a blocking file dialog or worker call holds no frame borrows.
/// The producing UI never touches `Document` / `Theme` / `Engine` directly;
/// it returns one of these and [`App::handle_command`] actions it.
#[derive(Clone, Debug)]
pub(crate) enum AppCommand {
    NewDocument,
    LoadDocument,
    /// Save to the current file, or prompt (Save As) if there isn't one.
    SaveDocument,
    /// Always prompt for a destination.
    SaveDocumentAs,
    /// Set the theme preference: `System` follows the OS light/dark
    /// setting, `Dark`/`Light` pin a palette. Persisted to config.
    SetTheme(ThemeChoice),
    /// Export the active subgraph (plus its local-def dependencies) to a
    /// file. No-op when the active tab isn't a subgraph.
    ExportSubgraph,
    /// Import a subgraph bundle from a file into the current document.
    ImportSubgraph,
    /// Publish a copy of the active subgraph into the shared library
    /// (`Library`), so it can be instanced as `Linked` anywhere. No-op
    /// when the active tab / selection isn't a subgraph.
    PromoteSubgraph,
    /// Publish a specific node's local subgraph def to the library (the
    /// S-badge "Publish" action). Updates the library def it came from
    /// in place when linked (`origin`), else creates a new entry and
    /// links the local def to it.
    PublishNodeSubgraph {
        node_id: NodeId,
    },
    /// Open a file dialog (filtered by `config`) for a node's `FsPath`
    /// const input, applying the chosen path as a `SetInput` edit. Raised
    /// by the inline pick button (see `gui::node::emit_path_picks`); the
    /// blocking dialog runs outside the record like the other file ops.
    PickInputPath {
        node_id: NodeId,
        port_idx: usize,
        config: Arc<FsPathConfig>,
    },
    /// Evaluate the graph once on the worker.
    Run,
    /// Request cancellation of the in-flight run.
    CancelRun,
    /// Start the worker's event loop (fire emitter events → run subscribers).
    StartEvents,
    /// Stop the worker's event loop.
    StopEvents,
    /// Open (or focus) the Config tab — the app-settings window.
    OpenConfig,
    /// Open an ONNX file dialog for one of the ML model paths and persist
    /// the choice. Raised by the Config tab's "Browse…" buttons.
    PickMlModel(MlModelKind),
    /// Set an ML model path directly from the Config tab's editable field
    /// (a typed or pasted path), then persist + republish it.
    SetMlModelPath {
        kind: MlModelKind,
        path: PathBuf,
    },
    /// Toggle whether launch reopens the last document. Raised by the
    /// Config tab's "Load last document on startup" checkbox; persisted.
    SetLoadLastDocument(bool),
}

/// Which ML model path an [`AppCommand::PickMlModel`] targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MlModelKind {
    /// The `ml_denoise` node's model (DeepSNR).
    Denoise,
    /// The `remove_stars` node's model (StarNet).
    StarRemoval,
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
    pub(crate) library: &'a Library,
    /// Last run's per-node state (status, logs, fetched runtime values),
    /// keyed by authoring `NodeId`. Read by the inspection panel's Log and
    /// Inputs/Outputs sections.
    pub(crate) run_state: &'a RunState,
    /// Persisted app config (theme + ML model paths), so a non-graph view
    /// like the Config tab can display the current settings.
    pub(crate) config: &'a AppConfig,
    /// Whether the worker's event loop is running — drives the events
    /// toggle's on/off look. App-side intent rather than the worker's atomic,
    /// so the button can't lag a frame behind (and `Update` from a one-shot
    /// run, which tears the loop down, resets it in lockstep).
    pub(crate) events_running: bool,
}

/// Thin shell around the [`Editor`] (which owns the document + its edit
/// pipeline + the GUI tree): `App` holds only the runtime/IO the editor
/// borrows each frame — the [`Engine`] (func lib + worker + script host),
/// the active theme, session config + file path, and the host handle. Its
/// `frame` drains the engine's worker + script queues into the editor's
/// projections, runs one `Editor::frame`, and actions the [`AppCommand`]
/// it surfaces (file / theme / subgraph dialogs, run) outside the record.
#[derive(Debug)]
pub(crate) struct App {
    pub(crate) editor: Editor,
    /// Shared runtime services — func lib + evaluation worker + script host
    /// — built at startup. The GUI loads an `engine.library` snapshot each
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
    /// Whether the worker's event loop is currently running (toggled by the
    /// events button). Reset whenever a one-shot run's `Update` tears the
    /// loop down, so it tracks the worker's real state.
    pub(crate) events_running: bool,
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
            events_running: false,
        };
        app.config = AppConfig::load();
        // Resolve the saved preference: `System` (the default) follows
        // the OS light/dark setting, re-queried each launch.
        app.theme = Theme::from_preset(app.config.theme.resolve());
        // Reopen the last document unless the user turned that off. A failed
        // load (the file moved or was deleted) clears the stale path, so the
        // next launch starts clean instead of retrying the broken path.
        if app.config.load_last_document
            && let Some(path) = app.config.document_path.clone()
            && !app.load_document(&path)
        {
            app.config.document_path = None;
            app.config.save();
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
        // The worker's `Update` tears down any running event loop, so the
        // toggle must drop in lockstep.
        self.events_running = false;
    }

    /// Start the worker's event loop on the current graph: emitter events
    /// fire and their subscribers run until stopped.
    pub(crate) fn start_events(&mut self) {
        self.engine
            .start_event_loop(self.editor.document.graph.clone());
        self.events_running = true;
    }

    /// Stop the worker's event loop.
    pub(crate) fn stop_events(&mut self) {
        self.engine.stop_event_loop();
        self.events_running = false;
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
                WorkerEvent::ExecutionFinished(Ok(stats)) => {
                    if stats.cancelled {
                        tracing::info!(
                            "run cancelled after {} node(s)",
                            stats.executed_nodes.len()
                        );
                    }
                    run_state.set_results(&stats);
                }
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
                ScriptMessage::Apply(intents) => {
                    let library = self.engine.library.load();
                    self.editor.apply_external_intents(intents, &library);
                }
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

        // While nodes are computing, keep repainting (~20 fps) so the running
        // node's live elapsed-so-far timer ticks — a single long node emits no
        // progress events between its start and finish.
        if self.editor.run_state.is_running() {
            ui.request_repaint_after(Duration::from_millis(50));
        }

        // Apply anything scripts pushed since the last frame (graph edits,
        // run, quit) before the editor rebuilds, so the scene reflects them.
        self.handle_script_inbound();

        // One consistent library snapshot for the whole frame (cheap atomic
        // load); a mid-frame promote/publish swap takes effect next frame.
        let library = self.engine.library.load();
        let command = self.editor.frame(
            ui,
            &library,
            &self.theme,
            self.config.theme,
            &self.config,
            self.events_running,
            &self.host_handle,
        );

        // A disk-cache toggle this frame: flush the node's resident value to disk now
        // (a `SaveCaches` to the worker — refresh the program + persist, no re-run),
        // instead of waiting for the next evaluation.
        if self.editor.take_caches_dirty() {
            self.engine.save_caches(self.editor.document.graph.clone());
        }

        // The frame settled which inspector panels are open; request the
        // runtime values for any that still need them.
        self.request_open_panel_values();

        // Menu side effects run last so the blocking file dialog opens
        // after the frame's record + drain. Loading replaces the
        // document/theme wholesale, so always relayout afterward.
        if let Some(command) = command {
            self.handle_command(ui, command);
            ui.request_relayout();
        }
    }
}
