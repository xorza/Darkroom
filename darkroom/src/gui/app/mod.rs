use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use palantir::Ui;
use scenarium::Binding;
use scenarium::ConstValue;
use scenarium::FsPathMode;
use scenarium::NodeId;

use crate::core::document::open_document::OpenDocument;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::io::preferences::{Preferences, WindowState};
use crate::core::runtime_host::RuntimeHost;
use crate::core::status::StatusLog;
use crate::core::wake::Wake;
use crate::gui::HostHandle;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::commands::prefs::MlModelKind;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::app::discard_dialog::{DiscardChoice, DiscardOutcome};
use crate::gui::dialogs;
use crate::gui::pane::graph::node::port_row::PathPick;
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
/// evaluating it, and lends the document to the [`Session`] that authors each
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
    /// The frame's request queue, lent to [`Session::frame`] and drained of
    /// its app tier here once the pass is over. Lives on `App` because both
    /// levels drain it: the editor takes what the document owns, and what is
    /// left is ours. Carries no state between frames — a field only so the
    /// allocation is reused.
    requests: Requests,
}

/// A transition that replaces or discards the open document. Held while
/// the unsaved-changes prompt is up, then carried out (or dropped) by the
/// answer.
///
/// The two open variants differ only in where the path comes from:
/// [`Self::OpenPicked`] runs the file dialog *after* the prompt clears, so a
/// cancelled prompt doesn't leave the user having chosen a file for nothing,
/// while [`Self::OpenAt`] already holds one handed in from outside the editor
/// (Finder, via [`crate::platform`]). Carrying that path is what
/// costs this `Copy`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingTransition {
    Quit,
    New,
    OpenPicked,
    // Only `platform::macos` hands a path in; the other two OSes get theirs
    // through argv at launch, before an `App` exists to guard.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    OpenAt(PathBuf),
}

impl PendingTransition {
    /// How the prompt finishes "Save changes to X before …?".
    fn prompt_tail(&self) -> &'static str {
        match self {
            Self::Quit => "quitting",
            Self::New => "closing it",
            Self::OpenPicked | Self::OpenAt(_) => "opening another document",
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
        ui.set_theme(app.theme.palantir_theme.clone());
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
            PendingTransition::OpenPicked => self.load_picked_document(),
            PendingTransition::OpenAt(path) => self.load_document(&path),
        }
    }

    /// Open `path`, prompting first if the current document has unsaved
    /// edits. The entry point for a document the editor did not ask for —
    /// today, one Finder handed us, which makes this macOS-only: see
    /// [`crate::platform::route_opened_documents`] for why no other OS has a
    /// caller.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn open_document_at(&mut self, path: PathBuf) {
        self.guard_discard(PendingTransition::OpenAt(path));
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
        // Only the tail is taken, so the borrow of the pending transition ends
        // here — `apply_discard_outcome` below needs `&mut self`.
        let Some(tail) = self
            .confirm_discard
            .as_ref()
            .map(PendingTransition::prompt_tail)
        else {
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
        let outcome = discard_dialog::show(ui, file_name, tail);
        if outcome.choice != DiscardChoice::Stay {
            self.apply_discard_outcome(outcome);
        }
    }

    /// Open a file dialog for a node's `FsPath` const input and, if the
    /// user makes a selection, apply the chosen paths as a `SetInput` edit. Runs after
    /// authoring, so it goes through `Editor::apply_edit` rather than the
    /// frame's intent drain.
    ///
    /// Reports the edit's relayout need rather than acting on it — this runs
    /// after `Editor::frame` has handed its own back, and `App::frame` spends
    /// both together.
    #[must_use]
    fn pick_input_path(&mut self, pick: PathPick) -> Relayout {
        let extensions: Vec<&str> = pick.config.extensions.iter().map(String::as_str).collect();
        let value = match pick.config.mode {
            FsPathMode::ExistingFile => dialogs::pick_existing_file(&extensions)
                .map(|path| ConstValue::FsPath(path.to_string_lossy().into_owned())),
            FsPathMode::ExistingFiles => dialogs::pick_existing_files(&extensions).map(|paths| {
                ConstValue::FsPaths(
                    paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                )
            }),
            FsPathMode::NewFile => dialogs::pick_new_file(&extensions)
                .map(|path| ConstValue::FsPath(path.to_string_lossy().into_owned())),
            FsPathMode::Directory => dialogs::pick_directory()
                .map(|path| ConstValue::FsPath(path.to_string_lossy().into_owned())),
        };
        let Some(value) = value else {
            return Relayout::NotNeeded;
        };
        self.session.open.apply_edit(GraphIntent::SetInput {
            input: pick.port,
            to: Some(Binding::Const(value)),
        })
    }

    /// Prompt for a project file and load it. The
    /// [`PendingTransition::OpenPicked`] body: the picker runs here, *after*
    /// the guard cleared, so cancelling the unsaved-changes prompt never costs
    /// the user a file choice.
    pub(crate) fn load_picked_document(&mut self) {
        if let Some(path) = dialogs::pick_project_open_path(self.session.open.path.as_deref()) {
            self.load_document(&path);
        }
    }

    /// Replace the document with an empty one.
    pub(crate) fn new_document(&mut self) {
        self.adopt_document(OpenDocument::default());
    }

    /// Swap in `open` and reset every piece of state derived from the
    /// document it replaces.
    ///
    /// A fresh [`Session`] covers what it owns in one move: empty undo history
    /// (restoring the old doc via Cmd-Z would replay intents that no longer
    /// match the live tree), dropped gesture state, forced scene rebuild.
    ///
    /// The run projections are `App`'s, so they are cleared here rather than
    /// falling out of that — and they have to be: node ids are *persisted*, so
    /// reopening a document would otherwise reattach the previous session's
    /// statuses, timings, logs and preview images to nodes that have not run.
    /// [`RunState::clear`](crate::gui::state::run_state::RunState::clear) drops
    /// exactly the document-derived half and leaves
    /// the worker-stream half (`compiled`, `activity`, `cache_ram`) standing —
    /// an in-flight run still reports against the program the worker
    /// acknowledged. What makes that half true again is the worker itself: the
    /// program and its whole runtime cache go with the document, and the
    /// `Cleared` acknowledgement is what resets the projection.
    ///
    /// The worker's disk cache repoints too, so disk-backed nodes read the new
    /// document's store rather than the old one's.
    fn adopt_document(&mut self, open: OpenDocument) {
        self.run_state.clear();
        self.runtime.clear_program();
        // One assignment, not two: the UI showing a document is replaced with
        // it, so there is no window where fresh panes hold a stale graph's
        // gesture state.
        self.session = Session::new(open);
        self.runtime
            .set_document_cache(self.session.open.path.as_deref());
        self.remember_document_path();
    }

    /// Load `path` into a fresh editor. A missing or corrupt file leaves the
    /// open document intact and surfaces its reason in the status bar.
    pub(crate) fn load_document(&mut self, path: &Path) {
        let open = match OpenDocument::load(path.to_path_buf()) {
            Ok(open) => open,
            Err(err) => {
                self.status.error(format!("load failed: {err:#}"));
                return;
            }
        };
        self.adopt_document(open);
        self.status.error = None;
    }

    /// Cmd+S: overwrite the current file if there is one, else fall
    /// back to Save As (first save of a fresh document).
    pub(crate) fn save_current(&mut self) {
        match self.session.open.path.clone() {
            Some(path) => self.save_document(&path),
            None => self.save_document_as(),
        }
    }

    /// Cmd+Shift+S / "Save As…": always prompt for a destination.
    fn save_document_as(&mut self) {
        if let Some(path) = dialogs::pick_project_save_path(self.session.open.path.as_deref()) {
            self.save_document(&path);
        }
    }

    /// Write the document to `path` and adopt it as the save target. Save-As
    /// moves the document, so the worker's disk cache repoints to the new
    /// location's store — the old one stays where it is.
    fn save_document(&mut self, path: &Path) {
        match self.session.open.save_to(path) {
            Ok(()) => {
                self.runtime
                    .set_document_cache(self.session.open.path.as_deref());
                self.remember_document_path();
                self.status.error = None;
            }
            Err(err) => self.status.error(format!("save failed: {err:#}")),
        }
    }

    /// Mirror the open document's active path into persisted preferences
    /// after a successful document lifecycle transition.
    fn remember_document_path(&mut self) {
        self.preferences.document_path = self.session.open.path.clone();
        self.save_preferences();
    }

    /// Re-derive everything that depends on [`Preferences`] and persist it.
    ///
    /// [`Preferences`]: crate::core::io::preferences::Preferences
    fn apply_preferences(&mut self, ui: &mut Ui) {
        self.theme = Theme::from_preset(self.preferences.theme.resolve());
        ui.set_theme(self.theme.palantir_theme.clone());
        self.runtime.configure_ml_model_defaults(&self.preferences);
        self.save_preferences();
    }

    /// Persist the preferences, surfacing a failed write in the status
    /// bar — the one save path every caller routes through, so a broken
    /// preferences file can't fail silently.
    pub(crate) fn save_preferences(&mut self) {
        if let Err(err) = self.preferences.save() {
            self.status.error(err);
        }
    }

    fn pick_ml_model(&mut self, kind: MlModelKind) {
        if let Some(path) = dialogs::pick_existing_file(&["onnx"]) {
            self.set_ml_model_path(kind, path);
        }
    }

    fn set_ml_model_path(&mut self, kind: MlModelKind, path: PathBuf) {
        match kind {
            MlModelKind::Denoise => self.preferences.ml_models.denoise = path,
            MlModelKind::StarRemoval => self.preferences.ml_models.star_removal = path,
        }
        self.runtime.configure_ml_model_defaults(&self.preferences);
        self.save_preferences();
    }

    /// Persist whether discarding unsaved changes prompts to save.
    /// Shared by the Preferences checkbox (via `Changed`) and the prompt's
    /// "Don't ask again", which calls this directly.
    pub(crate) fn set_confirm_unsaved(&mut self, on: bool) {
        self.preferences.confirm_unsaved_changes = on;
        self.save_preferences();
    }

    /// Compile the document graph and execute its sinks once on the
    /// worker. A compile error is reported to the engine's status log
    /// synchronously — no run starts, so the prior run's status stays
    /// untouched. Worker status reports acknowledge actual execution and
    /// event-loop transitions.
    pub(crate) fn run_graph(&mut self) {
        self.runtime
            .run_once(self.session.graph(), &mut self.status);
    }

    /// Like [`Self::run_graph`], but seeds the run at one node: only its
    /// upstream cone executes and its outputs are delivered.
    fn run_node(&mut self, node_id: NodeId) {
        // A node inside a local definition has no enclosing instance path,
        // so no execution seed resolves. The UI gates the play chip and the
        // menu action on `NodeCtx::runnable`, which is false there —
        // reaching this is a gating bug, not user input, so refuse rather
        // than kill the editor from a live command handler. Tested against
        // the *node's* graph, not the focused pane's: with several graph
        // panes open, a root node's chip stays valid while focus sits
        // elsewhere.
        if self.session.graph().find(node_id).is_none() {
            debug_assert!(false, "run-node reached for a node outside the root graph");
            return;
        }
        self.runtime
            .run_node(self.session.graph(), node_id, &mut self.status);
    }

    /// Evict one node's cache cone and project the outcome onto exactly the
    /// nodes it reaches. An empty answer means nothing was dispatched — a
    /// failed compile, or a node the program holds no work for — so there is
    /// nothing to project either.
    fn evict_cache(&mut self, node_id: NodeId) {
        let evicted = self
            .runtime
            .evict_cache(self.session.graph(), node_id, &mut self.status);
        if !evicted.is_empty() {
            self.run_state.clear_cache_projections(&evicted);
        }
    }

    /// Publish this node's resident value to the disk store. Nothing on screen
    /// changes — the value stays exactly where it was, and only gains a copy on
    /// disk — so unlike the eviction beside it, no projection is reset.
    fn flush_cache(&mut self, node_id: NodeId) {
        self.runtime
            .flush_cache(self.session.graph(), node_id, &mut self.status);
    }

    /// Start the worker's event loop on the current graph: emitter events
    /// fire their subscribers until stopped. A compile error (reported to
    /// the engine's status log) leaves the loop's running state as it was —
    /// nothing reached the worker.
    fn start_events(&mut self) {
        self.runtime
            .start_event_loop(self.session.graph(), &mut self.status);
    }

    /// Stop the worker's event loop.
    fn stop_events(&mut self) {
        self.runtime.stop_event_loop();
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
        // while a command runs, which is what lets `AppCommand::apply` take all
        // of `self`.
        while let Some(command) = self.requests.pop_app() {
            needs_relayout |= command.apply(self, ui);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A document Finder hands over is guarded exactly like one the user
    /// picked: same question, same wording. The two differ only in where the
    /// path came from, and the prompt has no business exposing that.
    #[test]
    fn an_externally_opened_document_prompts_like_a_picked_one() {
        let picked = PendingTransition::OpenPicked;
        let handed = PendingTransition::OpenAt(PathBuf::from("/tmp/scene.darkroom"));
        assert_eq!(picked.prompt_tail(), "opening another document");
        assert_eq!(handed.prompt_tail(), picked.prompt_tail());

        // The other two stay distinct — a shared tail would misdescribe what
        // the user is about to lose the document to.
        assert_eq!(PendingTransition::Quit.prompt_tail(), "quitting");
        assert_eq!(PendingTransition::New.prompt_tail(), "closing it");
    }

    /// The path rides along rather than being re-picked after the prompt, so
    /// answering "Save" opens the document the user actually double-clicked.
    #[test]
    fn a_handed_in_transition_carries_its_path_through_the_prompt() {
        let path = PathBuf::from("/tmp/from-finder.darkroom");
        let pending = PendingTransition::OpenAt(path.clone());
        let held = pending.clone();
        assert_eq!(held, PendingTransition::OpenAt(path));
        assert_eq!(held, pending, "surviving the guard costs the path nothing");
    }
}
