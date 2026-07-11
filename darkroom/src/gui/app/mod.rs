use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aperture::Ui;
use scenarium::graph::Graph as CoreGraph;
use scenarium::library::Library;

use crate::core::document::Document;
use crate::core::engine::Engine;
use crate::core::io::preferences::{Preferences, WindowState};
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::worker::WorkerEvent;
use crate::gui::HostHandle;
use crate::gui::MAIN_WINDOW;
use crate::gui::run_state::RunState;
use crate::gui::theme::Theme;

pub(crate) mod commands;
pub(crate) mod editor;
mod exit_dialog;

use editor::Editor;
use exit_dialog::ExitChoice;

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
    /// The last failed action's message (the engine's
    /// [`StatusLog::error`](crate::core::status::StatusLog) slot), shown in
    /// the status bar until a subsequent success clears it.
    pub(crate) status_error: Option<&'a str>,
}

/// Thin shell around the [`Editor`] (which owns the document + its edit
/// pipeline + the GUI tree): `App` holds only the runtime/IO the editor
/// borrows each frame — the [`Engine`] (func lib + worker + script host),
/// the active theme, session preferences + file path, and the host handle. Its
/// `frame` drains the engine's worker + script queues into the editor's
/// projections, runs one `Editor::frame`, and actions the
/// [`AppCommand`](commands::AppCommand) it surfaces (file / theme /
/// subgraph dialogs, run) outside the record.
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
    /// last document) and push the resolved aperture theme onto `Ui`.
    /// Restore failures degrade silently to defaults — a missing or
    /// corrupt preferences, or a deleted document, must not block launch.
    ///
    /// Handed to [`aperture::WinitHost::run`], which calls it once the
    /// `Ui` + [`HostHandle`] exist (before the first frame).
    pub(crate) fn new(
        ui: &mut Ui,
        handle: HostHandle,
        script_cfg: ScriptConfig,
        preferences: Preferences,
    ) -> Self {
        let mut document: Document = CoreGraph::default().into();
        document.main_view.auto_layout_default(&document.graph);
        // The worker + script host wake the winit loop via the host handle;
        // the headless/tui drivers swap in a tokio `Notify` (see
        // `crate::core::wake`).
        let wake: Wake = {
            let handle = handle.clone();
            Arc::new(move || handle.request_repaint(MAIN_WINDOW))
        };
        // `preferences` is loaded in `run_gui` (before the window exists, so
        // its saved geometry can size the window at creation) and handed in
        // here — the `Preferences::load()` there already published the ML
        // model paths into lens.
        let mut app = Self {
            editor: Editor::new(document),
            engine: Engine::new(&script_cfg, wake),
            theme: Theme::default(),
            host_handle: handle,
            current_path: None,
            preferences,
            events_running: false,
            confirm_quit: false,
        };
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
            app.save_preferences();
        }
        // Resolved theme (default, or whatever the preferences restored)
        // onto the Ui so aperture widgets paint correctly frame 1.
        ui.theme = app.theme.aperture_theme.clone();
        // ui.debug_overlay.damage_rect = true;
        app
    }

    /// Consume worker results posted since the last frame. A finished run
    /// reprojects per-node `ExecStatus` (the status glow) and per-node
    /// logs (the inspector's Log section); a failed run clears both and
    /// surfaces in the status bar. An argument-value reply lands in the run
    /// state (uploading any preview textures via `ui`). Drained before the
    /// editor's scene rebuild so they reflect the latest run.
    fn drain_worker_events(&mut self, ui: &Ui) {
        // Collect to drop the channel borrow before the status writes below
        // (both live on `self.engine`).
        let events: Vec<WorkerEvent> = self.engine.drain_worker().collect();
        for event in events {
            let run_state = &mut self.editor.run_state;
            match event {
                WorkerEvent::ExecutionFinished(Ok(stats)) => {
                    if stats.cancelled {
                        tracing::info!(
                            "run cancelled after {} node(s)",
                            stats.executed_nodes.len()
                        );
                    }
                    // The stats' flat ids project through the compile-phase
                    // flatten map the engine kept when it sent this run.
                    run_state.set_results(&stats, &self.engine.flatten_map);
                    // A finished run supersedes any lingering failure message
                    // (e.g. an earlier event-loop tick's), so the loop
                    // self-heals in the status bar too.
                    self.engine.status.error = None;
                }
                WorkerEvent::ExecutionFinished(Err(err)) => {
                    run_state.clear();
                    self.engine.status.error(format!("run failed: {err}"));
                }
                WorkerEvent::NodeProgress(progress) => run_state.apply_progress(&progress),
                WorkerEvent::ArgumentValues { request, values } => {
                    run_state.ingest_values(ui, request, values)
                }
            }
        }
    }

    /// Forward the frame's pending value requests to the worker: the
    /// editor's frame registered every value-showing surface (inspector
    /// panels, image-viewer tabs) into the run state's watch registry;
    /// drain it here, after the record. The reply arrives on a later
    /// frame's drain.
    fn request_watched_values(&mut self) {
        for req in self.editor.run_state.take_requests() {
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
                ScriptMessage::Print { msg } => self.engine.status.info(format!("script: {msg}")),
                ScriptMessage::Apply(intents) => {
                    let library = self.engine.library().clone();
                    self.editor.apply_external_intents(intents, &library);
                }
                ScriptMessage::RunOnce => run = true,
                // Shutdown is terminal: quit and drop the rest of the batch
                // (the app is closing, so any remaining edits/runs are moot).
                ScriptMessage::Shutdown => {
                    self.quit();
                    return;
                }
            }
        }
        // Coalesce: many `run()`s in one drain still kick a single run.
        if run {
            self.run_graph();
        }
    }

    /// Mirror the window's live geometry into the persisted preferences
    /// (in memory only). Called each frame so any later `preferences.save()`
    /// — on quit — writes the current size / position. Size and position
    /// are refreshed only while the window is floating; a maximized window
    /// keeps its last floating geometry so un-maximizing on the next launch
    /// lands at the right size.
    fn track_window_state(&mut self, ui: &Ui) {
        let geom = ui.window_geometry();
        match &mut self.preferences.window {
            Some(w) => {
                w.maximized = geom.maximized;
                if !geom.maximized {
                    w.size = geom.inner_size;
                    w.position = geom.outer_position;
                }
            }
            None => {
                self.preferences.window = Some(WindowState {
                    size: geom.inner_size,
                    maximized: geom.maximized,
                    position: geom.outer_position,
                });
            }
        }
    }

    /// Persist preferences (including the window geometry mirrored by
    /// [`Self::track_window_state`]) and ask the host to exit. Every
    /// explicit quit path routes through here so geometry is saved on the
    /// way out; the titlebar-X clean close — which never calls this —
    /// saves in [`Self::handle_exit`] instead.
    fn quit(&mut self) {
        self.save_preferences();
        self.host_handle.quit();
    }

    /// Resolve a pending quit. A window-close request (titlebar X) with
    /// unsaved changes raises the confirm dialog and vetoes the close
    /// ([`Ui::keep_open`]); a clean document lets the close proceed. While
    /// the dialog is up, render it and act on the choice. Also finishes
    /// File ▸ Quit, which set `confirm_quit` via `request_quit` earlier this
    /// frame.
    fn handle_exit(&mut self, ui: &mut Ui) {
        if ui.close_requested() {
            // The titlebar X closes the window after this frame unless we
            // veto. A clean close never routes through `quit`, so persist
            // geometry here — `track_window_state` already mirrored the
            // current size / position into `preferences` this frame.
            self.save_preferences();
            // Unsaved changes raise the prompt and veto the close — unless
            // the user turned confirmation off, in which case it proceeds.
            if self.editor.dirty && self.preferences.confirm_unsaved_on_exit {
                ui.keep_open();
                self.confirm_quit = true;
            }
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
                self.quit();
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
                    self.quit();
                }
            }
        }
    }
}

impl aperture::App for App {
    fn frame(&mut self, _win: aperture::WindowToken, ui: &mut Ui) {
        // Keep the persisted window geometry current so a save on quit
        // captures the latest size / position.
        self.track_window_state(ui);

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
        let library = self.engine.library().clone();
        let command = self.editor.frame(
            ui,
            &library,
            &self.theme,
            &mut self.preferences,
            self.events_running,
            self.engine.status.error.as_deref(),
        );

        // A disk-cache toggle this frame: flush the node's resident value to disk now
        // (a `SaveCaches` to the worker — refresh the program + persist, no re-run),
        // instead of waiting for the next evaluation. A compile failure is
        // reported to the engine's status log; nothing is sent.
        if self.editor.take_caches_dirty() {
            self.engine.save_caches(&self.editor.document.graph);
        }

        // The frame registered everything watching runtime values; request
        // any that still need fetching this epoch.
        self.request_watched_values();

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
