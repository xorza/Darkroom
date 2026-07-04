use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use palantir::Ui;
use scenarium::data::FsPathConfig;
use scenarium::graph::{Graph as CoreGraph, NodeId};
use scenarium::library::Library;

use crate::core::document::Document;
use crate::core::engine::Engine;
use crate::core::io::preferences::Preferences;
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::worker::WorkerEvent;
use crate::gui::HostHandle;
use crate::gui::MAIN_WINDOW;
use crate::gui::run_state::RunState;
use crate::gui::theme::Theme;

mod commands;
pub(crate) mod editor;
mod exit_dialog;

use editor::Editor;
use exit_dialog::ExitChoice;

/// A deferred, side-effecting command a UI surface (the menu bar, the graph
/// toolbar, the Preferences tab, a node's S-badge, an inline path-picker)
/// hands to [`App`] to perform *outside* the record pass — after the frame's
/// record + drain, so a blocking file dialog or worker call holds no frame
/// borrows. The producing UI never touches `Document` / `Theme` / `Engine`
/// directly; it returns one of these and [`App::handle_command`] dispatches
/// it to the matching group handler (one submodule of `gui::app::commands`
/// per variant here).
#[derive(Clone, Debug)]
pub(crate) enum AppCommand {
    /// Document file lifecycle — `commands::file`.
    File(FileCommand),
    /// Subgraph → library publishing — `commands::subgraph`.
    Subgraph(SubgraphCommand),
    /// Graph execution + worker event loop — `commands::run`.
    Run(RunCommand),
    /// Preferences edits — `commands::prefs`.
    Prefs(PrefsCommand),
    /// Node edits raised via a dialog — `commands::edit`.
    Edit(EditCommand),
    /// App shell: navigation + lifecycle — `commands::shell`.
    Shell(ShellCommand),
}

/// Document file lifecycle. Handled by `App::handle_file`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FileCommand {
    /// Replace the document with an empty one.
    New,
    /// Prompt for a file and load it.
    Load,
    /// Save to the current file, or prompt (Save As) if there isn't one.
    Save,
    /// Always prompt for a destination.
    SaveAs,
}

/// Publishing subgraphs into the shared library. Handled by
/// `App::handle_subgraph`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SubgraphCommand {
    /// Export the active subgraph (plus its local-def dependencies) to a
    /// file. No-op when the active tab isn't a subgraph.
    Export,
    /// Import a subgraph bundle from a file into the current document.
    Import,
    /// Publish a copy of the active subgraph into the shared library, so it
    /// can be instanced as `Linked` anywhere. No-op off a subgraph.
    Promote,
    /// Publish a node's local subgraph def to the library (the S-badge
    /// "Publish" action): update in place when linked, else create + link.
    PublishNode { node_id: NodeId },
}

/// Graph execution + the worker event loop. Handled by `App::handle_run`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RunCommand {
    /// Evaluate the graph once on the worker.
    Once,
    /// Request cancellation of the in-flight run.
    Cancel,
    /// Start the worker's event loop (emitter events → run subscribers).
    StartEvents,
    /// Stop the worker's event loop.
    StopEvents,
}

/// Preferences edits. Handled by `App::handle_prefs`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PrefsCommand {
    /// The Preferences tab edited a field of [`Preferences`] in place (any
    /// checkbox / radio / path field). `App` re-syncs derived state (theme
    /// palette, ML paths) and persists — one command for every field, so
    /// adding a preference needs no new command.
    Changed,
    /// Open an ONNX file dialog for one of the ML model paths (the "Browse…"
    /// buttons) — the blocking dialog runs outside the record, unlike the
    /// in-place field edits that report [`Self::Changed`].
    PickMlModel(MlModelKind),
}

/// Node edits that need a dialog before applying. Handled by
/// `App::handle_edit`.
#[derive(Clone, Debug)]
pub(crate) enum EditCommand {
    /// Open a file dialog (filtered by `config`) for a node's `FsPath`
    /// const input, applying the chosen path as a `SetInput` edit. Raised
    /// by the inline pick button (see `gui::node::emit_path_picks`).
    PickInputPath {
        node_id: NodeId,
        port_idx: usize,
        config: Arc<FsPathConfig>,
    },
}

/// App shell: navigation + lifecycle. Handled by `App::handle_shell`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShellCommand {
    /// Open (or focus) the Preferences tab — the app-settings window.
    OpenPreferences,
    /// Quit the app. Routed through `App::request_quit`, which prompts to
    /// save first if the document has unsaved changes.
    Quit,
}

/// Which ML model path a [`PrefsCommand::PickMlModel`] targets.
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
    pub(crate) library: &'a Library,
    /// Last run's per-node state (status, logs, fetched runtime values),
    /// keyed by authoring `NodeId`. Read by the inspection panel's Log and
    /// Inputs/Outputs sections.
    pub(crate) run_state: &'a RunState,
    /// Whether the worker's event loop is running — drives the events
    /// toggle's on/off look. App-side intent rather than the worker's atomic,
    /// so the button can't lag a frame behind (and `Update` from a one-shot
    /// run, which tears the loop down, resets it in lockstep).
    pub(crate) events_running: bool,
}

/// Thin shell around the [`Editor`] (which owns the document + its edit
/// pipeline + the GUI tree): `App` holds only the runtime/IO the editor
/// borrows each frame — the [`Engine`] (func lib + worker + script host),
/// the active theme, session preferences + file path, and the host handle. Its
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
    pub(crate) preferences: Preferences,
    /// Whether the worker's event loop is currently running (toggled by the
    /// events button). Reset whenever a one-shot run's `Update` tears the
    /// loop down, so it tracks the worker's real state.
    pub(crate) events_running: bool,
    /// Whether the "save changes before quitting?" dialog is currently up.
    /// Raised when a quit is requested (window close, File ▸ Quit) with
    /// unsaved changes; cleared when the user answers.
    confirm_quit: bool,
}

impl App {
    /// Build the app before the first frame: assemble the func lib +
    /// seed document, then restore persisted preferences (saved theme +
    /// last document) and push the resolved palantir theme onto `Ui`.
    /// Restore failures degrade silently to defaults — a missing or
    /// corrupt preferences, or a deleted document, must not block launch.
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
            preferences: Preferences::default(),
            events_running: false,
            confirm_quit: false,
        };
        app.preferences = Preferences::load();
        // Resolve the saved preference: `System` (the default) follows
        // the OS light/dark setting, re-queried each launch.
        app.theme = Theme::from_preset(app.preferences.theme.resolve());
        // Reopen the last document unless the user turned that off. A failed
        // load (the file moved or was deleted) clears the stale path, so the
        // next launch starts clean instead of retrying the broken path.
        if app.preferences.load_last_document
            && let Some(path) = app.preferences.document_path.clone()
            && !app.load_document(&path)
        {
            app.preferences.document_path = None;
            app.preferences.save();
        }
        // Resolved theme (default, or whatever the preferences restored)
        // onto the Ui so palantir widgets paint correctly frame 1.
        ui.theme = app.theme.palantir_theme.clone();
        // ui.debug_overlay.damage_rect = true;
        app
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

    /// Resolve a pending quit. A window-close request (titlebar X) with
    /// unsaved changes raises the confirm dialog and vetoes the close
    /// ([`Ui::keep_open`]); a clean document lets the close proceed. While
    /// the dialog is up, render it and act on the choice. Also finishes
    /// File ▸ Quit, which set `confirm_quit` via `request_quit` earlier this
    /// frame.
    fn handle_exit(&mut self, ui: &mut Ui) {
        // A window-close request (titlebar X) with unsaved changes raises
        // the prompt and vetoes the close — unless the user turned
        // confirmation off, in which case the close proceeds.
        if ui.close_requested() && self.editor.dirty && self.preferences.confirm_unsaved_on_exit {
            ui.keep_open();
            self.confirm_quit = true;
        }
        if !self.confirm_quit {
            return;
        }

        let file_name = self
            .current_path
            .as_deref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str());
        let outcome = exit_dialog::show(ui, file_name);
        match outcome.choice {
            ExitChoice::Stay => {}
            ExitChoice::Cancel => self.confirm_quit = false,
            ExitChoice::Discard => {
                if outcome.dont_ask_again {
                    self.set_confirm_exit(false);
                }
                self.host_handle.quit();
            }
            ExitChoice::Save => {
                self.confirm_quit = false;
                if outcome.dont_ask_again {
                    self.set_confirm_exit(false);
                }
                self.save_current();
                // Save As can be cancelled, leaving the doc dirty — only
                // quit once the save actually landed.
                if !self.editor.dirty {
                    self.host_handle.quit();
                }
            }
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
            &mut self.preferences,
            self.events_running,
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

        // Catch a window-close request and, with unsaved changes, prompt
        // instead of quitting. Runs after `handle_command` so a File ▸ Quit
        // (which set `confirm_quit` via `request_quit`) draws its dialog the
        // same frame.
        self.handle_exit(ui);
    }
}
