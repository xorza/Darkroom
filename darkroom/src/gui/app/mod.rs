use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use palantir::Ui;

use crate::core::document::open_document::OpenDocument;
use crate::core::io::preferences::{Preferences, WindowState};
use crate::core::runtime_host::RuntimeHost;
use crate::core::status::StatusLog;
use crate::core::wake::Wake;
use crate::gui::HostHandle;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::app::discard_dialog::{DiscardChoice, DiscardOutcome};
use crate::gui::relayout::Relayout;
use crate::gui::requests::Requests;
use crate::gui::state::process_memory::ProcessMemory;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

pub(crate) mod commands;
pub(crate) mod ctx;
mod discard_dialog;
pub(crate) mod session;

use session::Session;

/// The editor shell: it owns the open document and the runtime services
/// evaluating it, and lends the document to the [`Editor`] that authors each
/// frame. `App` also owns preferences, dialogs, theme, and exit policy.
/// `update` drains external queues once, while replayable `record` runs
/// `Editor::frame` and handles actions only in the pass that receives input.
#[derive(Debug)]
pub(crate) struct App {
    /// The document being edited and the UI showing it — replaced as a unit
    /// when a different file is opened, since neither outlives the other.
    session: Session,
    /// The func library, the evaluation worker, and the status log they
    /// report into — everything the document is executed against.
    runtime: RuntimeHost,
    /// The last completed run's per-node state, keyed by the document's
    /// `NodeId`s: execution status (the glow + header time), log lines, and
    /// the values preview cards and viewers read. Owned here because `App` is
    /// its only writer — it fills as the worker is drained — and lent to the
    /// frame through [`AppCtx`]. Off the serialized state.
    run_state: RunState,
    /// The user-facing outcome log behind the status bar's sticky error slot.
    ///
    /// On `App` rather than inside the runtime because most of what reports
    /// here is not the runtime's: file load/save outcomes, preferences
    /// failures, and the document restore at startup all write it, and only
    /// compile failures come from the worker side. Lent to whoever is
    /// reporting.
    status: StatusLog,
    theme: Theme,
    host_handle: HostHandle,
    /// Persisted session state (active theme name + last document).
    /// Written on every doc/theme change so the next launch reopens
    /// where the user left off.
    preferences: Preferences,
    /// The document-replacing transition waiting on the unsaved-changes
    /// prompt, and thus whether that prompt is up at all. Raised by
    /// [`Self::guard_discard`]; cleared when the user answers.
    confirm_discard: Option<PendingTransition>,
    /// Throttled sampler behind the status bar's `MEM` clause. Lives on
    /// `App` rather than `Editor` because it measures the process, not the
    /// document. Sampled where it is consumed rather than in `update`:
    /// [`ProcessMemory::sample`] refreshes at most once per interval, so a
    /// second record pass repeats the reading the first one drew.
    process_memory: ProcessMemory,
    /// The frame's request queue, lent to [`Editor::frame`] and drained of
    /// its app tier here once the pass is over. Lives on `App` because both
    /// levels drain it: the editor takes what the document owns, and what is
    /// left is ours. Carries no state between frames — a field only so the
    /// allocation is reused.
    requests: Requests,
}

/// A transition that replaces or discards the open document. Held while
/// the unsaved-changes prompt is up, then carried out (or dropped) by the
/// answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingTransition {
    Quit,
    New,
    Load,
}

impl PendingTransition {
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
    /// Build the app before the first frame: open the startup document —
    /// `document` when the command line named one, else the preferred one —
    /// assemble runtime services, and push the resolved palantir theme onto
    /// `Ui`. Document restore failures degrade to an empty document and are
    /// retained in the shared status log rather than blocking launch.
    ///
    /// Handed to [`palantir::WinitHost::run`], which calls it once the
    /// `Ui` + [`HostHandle`] exist (before the first frame).
    pub(crate) fn new(
        ui: &mut Ui,
        handle: HostHandle,
        mut preferences: Preferences,
        document: Option<PathBuf>,
    ) -> Self {
        // The worker wakes the winit loop via the host handle (see
        // `crate::core::wake`).
        let wake: Wake = {
            let handle = handle.clone();
            Arc::new(move || handle.request_repaint(MAIN_WINDOW))
        };
        // `preferences` is loaded in `run_gui` before the window exists, so
        // its saved geometry can size the window at creation.
        let mut runtime = RuntimeHost::new(wake, &preferences);
        let mut status = StatusLog::default();
        let open = OpenDocument::open_at_launch(document, &mut preferences, &mut status);
        runtime.set_document_cache(open.path.as_deref());
        let mut app = Self {
            session: Session::new(open),
            runtime,
            run_state: RunState::default(),
            status,
            theme: Theme::default(),
            host_handle: handle,
            preferences,
            confirm_discard: None,
            process_memory: ProcessMemory::new(),
            requests: Requests::default(),
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
        self.session.open.dirty && self.preferences.confirm_unsaved_changes
    }

    /// Carry out `transition`, or raise the unsaved-changes prompt first when
    /// the document holds edits worth protecting. Every path that would
    /// discard the open document routes through here — File ▸ New, File ▸
    /// Open, File ▸ Quit, ⌘Q — so the policy lives in one place instead of
    /// being restated (or forgotten) per caller.
    fn guard_discard(&mut self, transition: PendingTransition) {
        if self.needs_discard_confirmation() {
            self.confirm_discard = Some(transition);
        } else {
            self.perform(transition);
        }
    }

    /// Run a transition the guard cleared. `Load` picks its file here
    /// rather than before the prompt, so a cancelled prompt doesn't leave
    /// the user having chosen a file for nothing.
    fn perform(&mut self, transition: PendingTransition) {
        match transition {
            PendingTransition::Quit => self.quit(),
            PendingTransition::New => self.new_document(),
            PendingTransition::Load => self.load_picked_document(),
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
            self.confirm_discard = Some(PendingTransition::Quit);
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
        let resolution = outcome.resolve(self.session.open.dirty);
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
            .session
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

        // Everything derived from the run and the document, swept once a
        // frame — **the one place that happens**. All of these caches outlive
        // the scene on purpose (a closed tab must resolve its port centers the
        // frame it comes back), so none can decide for itself that an entry is
        // dead; only the document knows, and a node id is never reused.
        //
        // Two owners, so two calls: the run projection filters on the node
        // that published, and everything the window caches — canvas geometry,
        // open inspectors, per-tab viewer framing — goes through the session.
        // A new cache gets a line here rather than a third call site — see
        // `Document::holds_node`, which they all ask and which lists them.
        //
        // A node deleted later in the same frame's record is swept next frame.
        // Nothing reads a dead entry in between — the geometry is reached
        // through a `NodeCtx` that only resolves for live nodes, and
        // `draw_panels` skips a panel whose node is gone — so the lag costs
        // memory and nothing else.
        self.run_state.sync(
            &mut self.runtime,
            &mut self.status,
            ui,
            &self.session.open.document,
        );
        self.session.reconcile_caches();

        self.handle_close_request(ui);
    }

    fn record(&mut self, _win: palantir::WindowToken, ui: &mut Ui) {
        // While nodes are computing, keep repainting (~20 fps) so the running
        // node's live elapsed-so-far timer ticks — a single long node emits no
        // progress events between its start and finish.
        if self.run_state.activity.is_executing() {
            ui.request_repaint_after(Duration::from_millis(100));
        }

        // One library snapshot for this record pass (a cheap Arc clone).
        // A command that publishes below is visible to pass B or the next frame.
        let library = self.runtime.library.published.load();
        // The frame's read-only world, composed once here: everything below
        // derives its own context from this one rather than taking the refs
        // again.
        let ctx = AppCtx::new(
            &self.theme,
            &library,
            &self.run_state,
            StatusInputs {
                error: self.status.error.as_deref(),
                process_memory: self.process_memory.sample(Instant::now()),
            },
        );
        // Reset here rather than inside the frame: the queue is `App`'s, and
        // `record` runs twice on a frame carrying action input — the second
        // pass must not inherit what the first already ran.
        self.requests.clear();
        let mut needs_relayout =
            self.session
                .frame(ui, ctx, &mut self.preferences, &mut self.requests);
        // What the session left behind: every command the frame raised, in the
        // order it raised them — a keyboard chord and a click on the same
        // frame both land. Popped one at a time so the queue is not borrowed
        // while a command runs, which is what lets `handle_command` take all
        // of `self`.
        while let Some(command) = self.requests.pop_app() {
            needs_relayout |= self.handle_command(ui, command);
        }
        // The app's one relayout request, past both tiers: everything that can
        // strand `CanvasGeometry`'s cross-frame caches has reported by here,
        // and a single pass answers however many of them fired.
        if needs_relayout == Relayout::Needed {
            ui.request_relayout();
        }

        self.record_discard_prompt(ui);
    }
}
