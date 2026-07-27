use std::sync::Arc;
use std::time::Duration;

use palantir::Ui;
use scenarium::{Library, WorkerError, WorkerReport, WorkerStatusKind};

use crate::core::io::preferences::{Preferences, WindowState};
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::workspace::Workspace;
use crate::gui::HostHandle;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::discard_dialog::{DiscardChoice, DiscardOutcome};
use crate::gui::run_state::RunState;
use crate::gui::theme::Theme;

pub(crate) mod commands;
mod discard_dialog;
pub(crate) mod editor;

use editor::Editor;

/// Shared per-frame context threaded down the UI tree. Holds borrows
/// of state owned higher up so child subtrees don't take a growing
/// fan-out of `&` parameters.
#[derive(Copy, Clone, Debug)]
pub(crate) struct AppContext<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) library: &'a Library,
    /// Last run's centralized runtime state: per-node status/logs and the
    /// latest published values read by preview cards and viewers.
    pub(crate) run_state: &'a RunState,
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
    editor: Editor,
    workspace: Workspace,
    theme: Theme,
    host_handle: HostHandle,
    /// Persisted session state (active theme name + last document).
    /// Written on every doc/theme change so the next launch reopens
    /// where the user left off.
    preferences: Preferences,
    /// The document-replacing transition waiting on the unsaved-changes
    /// prompt, and thus whether that prompt is up at all. Raised by
    /// [`Self::guard_discard`]; cleared when the user answers.
    confirm_discard: Option<PendingAction>,
}

/// A transition that replaces or discards the open document. Held while
/// the unsaved-changes prompt is up, then carried out (or dropped) by the
/// answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingAction {
    Quit,
    New,
    Load,
}

impl PendingAction {
    /// How the prompt finishes "Save changes to X before …?".
    fn prompt_tail(self) -> &'static str {
        match self {
            Self::Quit => "quitting",
            Self::New => "closing it",
            Self::Load => "opening another document",
        }
    }
}

impl App {
    /// Build the app before the first frame: restore the preferred document,
    /// assemble runtime services, and push the resolved palantir theme onto
    /// `Ui`. Document restore failures degrade to an empty document and are
    /// retained in the shared status log rather than blocking launch.
    ///
    /// Handed to [`palantir::WinitHost::run`], which calls it once the
    /// `Ui` + [`HostHandle`] exist (before the first frame).
    pub(crate) fn new(
        ui: &mut Ui,
        handle: HostHandle,
        script_cfg: ScriptConfig,
        mut preferences: Preferences,
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
            workspace: Workspace::new(&script_cfg, wake, &mut preferences),
            theme: Theme::default(),
            host_handle: handle,
            preferences,
            confirm_discard: None,
        };
        // Resolve the saved preference: `System` (the default) follows
        // the OS light/dark setting, re-queried each launch.
        app.theme = Theme::from_preset(app.preferences.theme.resolve());
        // Resolved theme (default, or whatever the preferences restored)
        // onto the Ui so palantir widgets paint correctly frame 1.
        ui.theme = app.theme.palantir_theme.clone();
        // ui.debug_overlay.damage_rect = true;
        app
    }

    /// Consume worker results posted since the last frame. A completed run
    /// reprojects per-node `ExecStatus` (the status glow) and per-node
    /// logs (the inspector's Log section); a failed run clears both and
    /// surfaces in the status bar. A preview node's published value lands in
    /// the centralized preview store, which uploads its small thumbnail;
    /// visible cards and viewers read the new value during the frame
    /// already scheduled by the worker's wake callback. Drained before the
    /// editor's scene rebuild so they reflect the latest run.
    fn drain_worker_events(&mut self, ui: &Ui) {
        // Owned, so the channel borrow is gone before the status writes
        // below (both live on `self.workspace.runtime`).
        let events = self.workspace.runtime.drain_worker();
        for report in events {
            match report {
                WorkerReport::Installed(compiled) => {
                    self.editor.run_state.compiled = Some(compiled);
                }
                WorkerReport::Cleared => {
                    self.editor.run_state.compiled = None;
                    self.editor.run_state.clear();
                }
                WorkerReport::Error(error) => {
                    if matches!(&error, WorkerError::Execution { .. }) {
                        self.editor.run_state.clear();
                    }
                    self.workspace.runtime.status.error(error.to_string());
                }
                WorkerReport::Status(status) => {
                    if let WorkerStatusKind::Completed {
                        executed_node_count,
                        cancelled,
                        ..
                    } = status.kind
                    {
                        if cancelled {
                            tracing::info!("run cancelled after {executed_node_count} node(s)");
                        }
                        // A completed run supersedes any lingering failure message
                        // from an earlier event-loop tick.
                        self.workspace.runtime.status.error = None;
                    }
                    self.editor.run_state.apply_worker_status(&status);
                }
            }
        }
        // After the reports, so a value published during this run lands against
        // the compile the stream just acknowledged rather than the previous one.
        let previews = self.workspace.runtime.drain_previews();
        self.editor.run_state.ingest_previews(ui, previews);
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
                    let rejected = self
                        .editor
                        .commit_script_batch(&mut self.workspace.open, intents);
                    for reason in rejected {
                        self.workspace
                            .runtime
                            .status
                            .error(format!("script edit refused: {reason}"));
                    }
                }
                ScriptMessage::RunOnce => run = true,
                // Shutdown is terminal: quit and drop the rest of the batch
                // (the app is closing, so any remaining edits/runs are moot).
                // Deliberately not routed through `guard_discard`: an
                // automated client asked to exit, and a modal it can't
                // answer would hang the session. The interactive quit paths
                // still prompt.
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

    /// Whether a destructive transition has to prompt before proceeding:
    /// unsaved changes and the confirm preference both hold. The single
    /// predicate behind every path that replaces or discards the document.
    fn needs_discard_confirmation(&self) -> bool {
        self.editor.dirty && self.preferences.confirm_unsaved_changes
    }

    /// Carry out `action`, or raise the unsaved-changes prompt first when
    /// the document holds edits worth protecting. Every path that would
    /// discard the open document routes through here — File ▸ New, File ▸
    /// Open, File ▸ Quit, ⌘Q — so the policy lives in one place instead of
    /// being restated (or forgotten) per caller.
    fn guard_discard(&mut self, action: PendingAction) {
        if self.needs_discard_confirmation() {
            self.confirm_discard = Some(action);
        } else {
            self.perform(action);
        }
    }

    /// Run a transition the guard cleared. `Load` picks its file here
    /// rather than before the prompt, so a cancelled prompt doesn't leave
    /// the user having chosen a file for nothing.
    fn perform(&mut self, action: PendingAction) {
        match action {
            PendingAction::Quit => self.quit(),
            PendingAction::New => self.new_document(),
            PendingAction::Load => self.load_picked_document(),
        }
    }

    /// The titlebar X. The window stays open only while the prompt is up
    /// ([`Self::record_discard_prompt`] calls `keep_open`); with nothing to
    /// protect, the close proceeds untouched.
    fn handle_close_request(&mut self, ui: &Ui) {
        if !ui.close_requested() {
            return;
        }

        self.save_preferences();
        if self.needs_discard_confirmation() {
            self.confirm_discard = Some(PendingAction::Quit);
        }
    }

    /// Apply the prompt's answer: run the save it asked for, then let
    /// [`DiscardOutcome::resolve`] decide — against the dirty flag that
    /// save left behind — whether the transition goes through and whether
    /// the guard stays on.
    fn apply_discard_outcome(&mut self, outcome: DiscardOutcome) {
        let Some(pending) = self.confirm_discard.take() else {
            return;
        };
        if outcome.choice == DiscardChoice::Save {
            self.save_current();
        }
        let resolution = outcome.resolve(self.editor.dirty);
        if resolution.silence_prompt {
            self.set_confirm_unsaved(false);
        }
        if resolution.proceed {
            self.perform(pending);
        }
    }

    fn record_discard_prompt(&mut self, ui: &mut Ui) {
        let Some(pending) = self.confirm_discard else {
            return;
        };
        if ui.close_requested() {
            ui.keep_open();
        }
        let file_name = self
            .workspace
            .open
            .path
            .as_deref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str());
        let outcome = discard_dialog::show(ui, file_name, pending.prompt_tail());
        if outcome.choice != DiscardChoice::Stay {
            self.apply_discard_outcome(outcome);
        }
    }
}

impl palantir::App for App {
    fn update(&mut self, _win: palantir::WindowToken, ui: &Ui) {
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

    fn record(&mut self, _win: palantir::WindowToken, ui: &mut Ui) {
        // While nodes are computing, keep repainting (~20 fps) so the running
        // node's live elapsed-so-far timer ticks — a single long node emits no
        // progress events between its start and finish.
        if self.editor.run_state.activity.is_executing() {
            ui.request_repaint_after(Duration::from_millis(50));
        }

        // One library snapshot for this record pass (a cheap Arc clone).
        // A command that publishes below is visible to pass B or the next frame.
        let library = self.workspace.runtime.library.published.load();
        let command = self.editor.frame(
            &mut self.workspace.open,
            ui,
            &library,
            &self.theme,
            &mut self.preferences,
            self.workspace.runtime.status.error.as_deref(),
        );

        if let Some(command) = command {
            self.handle_command(ui, command);
        }

        self.record_discard_prompt(ui);
    }
}
