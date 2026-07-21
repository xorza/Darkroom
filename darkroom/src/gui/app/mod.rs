use std::sync::Arc;
use std::time::Duration;

use aperture::Ui;
use scenarium::Library;

use crate::core::io::preferences::{Preferences, WindowState};
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::worker::WorkerEvent;
use crate::core::workspace::Workspace;
use crate::gui::HostHandle;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::exit_dialog::{ExitChoice, ExitOutcome};
use crate::gui::run_state::RunState;
use crate::gui::theme::Theme;

pub(crate) mod commands;
pub(crate) mod editor;
mod exit_dialog;

use editor::Editor;

/// Shared per-frame context threaded down the UI tree. Holds borrows
/// of state owned higher up so child subtrees don't take a growing
/// fan-out of `&` parameters.
#[derive(Copy, Clone, Debug)]
pub(crate) struct AppContext<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) library: &'a Library,
    /// Last run's centralized runtime state: per-node status/logs and the
    /// latest pinned-output values read by previews and viewers.
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

/// GUI policy around the shared [`Workspace`] and the [`Editor`] that borrows
/// its open document. `App` owns preferences, dialogs, theme, and exit policy;
/// document/runtime coordination remains frontend-independent.
/// `update` drains external queues once, while replayable `record` runs
/// `Editor::frame` and handles actions only in the pass that receives input.
#[derive(Debug)]
pub(crate) struct App {
    pub(crate) editor: Editor,
    pub(crate) workspace: Workspace,
    pub(crate) theme: Theme,
    pub(crate) host_handle: HostHandle,
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
        // The worker + script host wake the winit loop via the host handle;
        // the headless/tui drivers swap in a tokio `Notify` (see
        // `crate::core::wake`).
        let wake: Wake = {
            let handle = handle.clone();
            Arc::new(move || handle.request_repaint(MAIN_WINDOW))
        };
        // `preferences` is loaded in `run_gui` before the window exists, so
        // its saved geometry can size the window at creation.
        let mut app = Self {
            editor: Editor::new(),
            workspace: Workspace::new(&script_cfg, wake, &preferences),
            theme: Theme::default(),
            host_handle: handle,
            preferences,
            events_running: false,
            confirm_quit: false,
        };
        // Resolve the saved preference: `System` (the default) follows
        // the OS light/dark setting, re-queried each launch.
        app.theme = Theme::from_preset(app.preferences.theme.resolve());
        if app.preferences.load_last_document
            && app.preferences.document_path.is_some()
            && app.workspace.open.path.is_none()
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
    /// surfaces in the status bar. A pinned output's live push lands in the
    /// centralized pinned-output store, which uploads its small image preview;
    /// visible previews and viewers read the new value during the frame
    /// already scheduled by the worker's wake callback. Drained before the
    /// editor's scene rebuild so they reflect the latest run.
    fn drain_worker_events(&mut self, ui: &Ui) {
        // Collect to drop the channel borrow before the status writes below
        // (both live on `self.workspace.runtime`).
        let events: Vec<WorkerEvent> = self.workspace.runtime.drain_worker().collect();
        for event in events {
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
                    self.editor
                        .run_state
                        .set_results(&stats, &self.workspace.runtime.flatten_map);
                    // A finished run supersedes any lingering failure message
                    // (e.g. an earlier event-loop tick's), so the loop
                    // self-heals in the status bar too.
                    self.workspace.runtime.status.error = None;
                }
                WorkerEvent::ExecutionFinished(Err(err)) => {
                    self.editor.run_state.clear();
                    self.workspace
                        .runtime
                        .status
                        .error(format!("run failed: {err}"));
                }
                WorkerEvent::NodeProgress(progress) => self
                    .editor
                    .run_state
                    .apply_progress(&progress, &self.workspace.runtime.flatten_map),
                WorkerEvent::PinnedOutputs(pinned) => {
                    self.editor.run_state.pinned_outputs.ingest(
                        ui,
                        pinned,
                        &self.workspace.open.document,
                    );
                }
            }
        }
    }

    /// Drain the script executor's inbound queue and act on each message:
    /// graph edits go through the editor's external-intent path (one batch
    /// = one undo entry), `run()` kicks one evaluation, `shutdown()` quits.
    /// Runs before the editor's frame so applied edits show the same frame.
    fn handle_script_inbound(&mut self) {
        let events = self.workspace.runtime.drain_script();
        let mut run = false;
        for event in events {
            match event {
                ScriptMessage::Print { msg } => {
                    self.workspace.runtime.status.info(format!("script: {msg}"))
                }
                ScriptMessage::Apply(intents) => {
                    let library = self.workspace.runtime.library.current.clone();
                    self.editor
                        .apply_external_intents(&mut self.workspace.open, intents, &library);
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
    /// saves in [`Self::handle_close_request`] instead.
    fn quit(&mut self) {
        self.save_preferences();
        self.host_handle.quit();
    }

    /// Whether a pending quit needs to prompt before proceeding: unsaved
    /// changes and the confirm-on-exit preference both hold. Shared by the
    /// titlebar-X path ([`Self::handle_close_request`]) and File ▸ Quit
    /// (`commands::shell::ShellCommand`'s handler), which both raise the
    /// same dialog off the same condition.
    fn needs_exit_confirmation(&self) -> bool {
        self.editor.dirty && self.preferences.confirm_unsaved_on_exit
    }

    fn handle_close_request(&mut self, ui: &Ui) {
        if !ui.close_requested() {
            return;
        }

        self.save_preferences();
        if self.needs_exit_confirmation() {
            self.confirm_quit = true;
        }
    }

    fn apply_exit_outcome(&mut self, outcome: ExitOutcome) {
        match outcome.choice {
            ExitChoice::Stay => unreachable!("exit outcome must resolve the dialog"),
            ExitChoice::Cancel => self.confirm_quit = false,
            ExitChoice::Discard => {
                self.confirm_quit = false;
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
                // Save As can be cancelled, leaving the document dirty.
                if !self.editor.dirty {
                    self.quit();
                }
            }
        }
    }

    fn record_exit(&mut self, ui: &mut Ui) {
        if ui.close_requested() && self.confirm_quit {
            ui.keep_open();
        }
        if !self.confirm_quit {
            return;
        }

        let file_name = self
            .workspace
            .open
            .path
            .as_deref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str());
        let outcome = exit_dialog::show(ui, file_name);
        if outcome.choice != ExitChoice::Stay {
            self.apply_exit_outcome(outcome);
        }
    }
}

impl aperture::App for App {
    fn update(&mut self, _win: aperture::WindowToken, ui: &Ui) {
        // Keep the persisted window geometry current so a save on quit
        // captures the latest size / position.
        self.track_window_state(ui);

        // Drain anything the worker posted since last frame, before the
        // editor rebuilds its scene so the status/log projections it
        // reads reflect the latest run.
        self.drain_worker_events(ui);

        // Apply anything scripts pushed since the last frame (graph edits,
        // run, quit) before the editor rebuilds, so the scene reflects them.
        self.handle_script_inbound();

        self.handle_close_request(ui);
    }

    fn record(&mut self, _win: aperture::WindowToken, ui: &mut Ui) {
        // While nodes are computing, keep repainting (~20 fps) so the running
        // node's live elapsed-so-far timer ticks — a single long node emits no
        // progress events between its start and finish.
        if self.editor.run_state.is_running() {
            ui.request_repaint_after(Duration::from_millis(50));
        }

        // One library snapshot for this record pass (a cheap Arc clone).
        // A command that publishes below is visible to pass B or the next frame.
        let library = self.workspace.runtime.library.current.clone();
        let command = self.editor.frame(
            &mut self.workspace.open,
            ui,
            &library,
            &self.theme,
            &mut self.preferences,
            self.events_running,
            self.workspace.runtime.status.error.as_deref(),
        );

        if self.editor.take_caches_dirty() {
            self.workspace.save_caches();
        }

        if let Some(command) = command {
            self.handle_command(ui, command);
        }

        self.record_exit(ui);
    }
}
