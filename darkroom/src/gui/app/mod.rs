use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use palantir::Ui;
use scenarium::Binding;
use scenarium::ConstValue;
use scenarium::FsPathMode;
use scenarium::Library;

use crate::core::edit::graph_intent::GraphIntent;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::preferences::preferences_store::{OpenedPreferences, PreferencesStore, Saved};
use crate::core::io::preferences::{Preferences, WindowState};
use crate::core::open_document::{Launched, Loaded, OpenDocument};
use crate::core::runtime_host::RuntimeHost;
use crate::core::status_log::{StatusFamily, StatusLog};
use crate::gui::app::app_host::AppHost;
use crate::gui::app::discard_dialog::{Answered, DiscardChoice, DiscardGuard, PendingTransition};
use crate::gui::app::driven_requests::DrivenRequests;
use crate::gui::app::error::DrivenOpenError;
use crate::gui::app::error::NonUnicodePath;
use crate::gui::app::session::Session;
use crate::gui::app::view_capture::{CaptureTarget, ViewCapture};
use crate::gui::dialogs::{Dialogs, SavedFile};
use crate::gui::frame::app_command::{
    AppCommand, EditCommand, FileCommand, MlModelKind, PathPick, PreferencesCommand, RunCommand,
    ViewCommand,
};
use crate::gui::frame::app_ctx::{AppCtx, StatusInputs};
use crate::gui::frame::frame_writes::FrameWrites;
use crate::gui::frame::relayout::Relayout;
use crate::gui::frame::requests::Requests;
use crate::gui::state::process_memory::ProcessMemory;
use crate::gui::state::run_state::{RunFeed, RunState};
use crate::gui::theme::Theme;

pub(crate) mod app_host;
mod discard_dialog;
pub(crate) mod driven_requests;
pub(crate) mod error;
pub(crate) mod offscreen_signals;
pub(crate) mod screenshot_ask;
pub(crate) mod session;
pub(crate) mod view_capture;

/// The editor shell: it owns the open document and the runtime services
/// evaluating it, and lends the document to the [`Session`] that authors each
/// frame. `App` also owns preferences, dialogs, theme, and exit policy.
/// `update` drains external queues once, while replayable `record` runs
/// `Session::frame` and handles actions only in the pass that receives input.
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
    /// The user-facing outcome log behind the status bar's sticky slot.
    ///
    /// On `App` rather than inside the runtime because most of what reports
    /// here is not the runtime's: file load/save outcomes, what a load
    /// repaired, preferences failures, and the document restore at startup all
    /// write it, and only run outcomes come from the worker side. Lent to
    /// whoever is reporting.
    status: StatusLog,
    theme: Theme,
    host: AppHost,
    /// Persisted session state: the last document, the window, the viewer
    /// and model choices. Written on every change through
    /// [`Self::save_preferences`], so the next launch reopens where the user
    /// left off.
    preferences: Preferences,
    /// Where [`Self::preferences`] persist: [`PreferencesStore::Memory`] when the file existed
    /// but could not be read, so the file stays as the user has it.
    preferences_store: PreferencesStore,
    /// Where path picks are answered: native dialogs, or a driver script.
    dialogs: Dialogs,
    /// The unsaved-changes prompt, and the transition it holds while it is up.
    discard: DiscardGuard,
    /// The image of the window a View command waits on; a second command replaces the first.
    capture: Option<ViewCapture>,
    /// What a driver on another thread asked of the next frames.
    driven: DrivenRequests,
    /// Raised at the end of a frame while a preview converts or a view capture waits for its
    /// image: work the frame does not show yet, which a driver on another thread waits out.
    pending_work: Arc<AtomicBool>,
    /// Throttled sampler behind the status bar's `MEM` clause. Lives on
    /// `App` rather than `Session` because it measures the process, not the
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

/// What a driver's open does with unsaved edits, in place of the prompt a person answers.
/// `darkroom/AGENTS.md`, "The editor harness", "A project opens in place".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnsavedEdits {
    /// Refuse the open: in the window, the person at it may own them.
    Refuse,
    /// Discard them: no one but the driver edits an offscreen editor.
    Discard,
}

/// What the app is launched with, read before its window exists.
#[derive(Debug)]
pub(crate) struct LaunchInputs {
    /// The preferences as their store opened them, where they persist, and why the file did not
    /// load, when it did not.
    pub(crate) preferences: OpenedPreferences,
    /// The document the command line named, if it named one.
    pub(crate) document: Option<PathBuf>,
    /// Where path picks are answered.
    pub(crate) dialogs: Dialogs,
}

impl App {
    /// Build the app before the first frame: open the startup document — the `launch` document when
    /// the command line named one, else the preferred one — assemble runtime services, and push the
    /// resolved palantir theme onto `Ui`. Document restore failures degrade to an empty document
    /// and are retained in the shared status log rather than blocking launch.
    ///
    /// Handed to [`palantir::WinitHost::run`], which calls it once the
    /// `Ui` + host handle exist (before the first frame); the driver calls it over its own host.
    pub(crate) fn new(ui: &mut Ui, host: AppHost, launch: LaunchInputs) -> Self {
        let LaunchInputs {
            preferences:
                OpenedPreferences {
                    preferences,
                    store: preferences_store,
                    failure,
                },
            document,
            dialogs,
        } = launch;
        let wake = host.wake();
        // `preferences` is loaded in `run` before the window exists, so
        // its saved geometry can size the window at creation.
        let mut status = StatusLog::default();
        if let Some(failure) = failure {
            status.error(
                StatusFamily::Preferences,
                format!("{failure}; this session's settings are not saved"),
            );
        }
        let run_state = RunState::new(Arc::clone(&wake));
        let mut runtime = RuntimeHost::new(wake, &preferences.ml_models);
        let remembered = preferences
            .document_path
            .as_deref()
            .filter(|_| preferences.load_last_document);
        let Launched {
            open,
            forgot_remembered,
        } = OpenDocument::open_at_launch(document, remembered, &mut status, runtime.library.cur());
        runtime.set_document_cache(open.path.as_deref());
        let mut app = Self {
            session: Session::new(open),
            runtime,
            run_state,
            status,
            theme: Theme::default(),
            host,
            preferences,
            preferences_store,
            dialogs,
            discard: DiscardGuard::default(),
            capture: None,
            driven: DrivenRequests::default(),
            pending_work: Arc::default(),
            process_memory: ProcessMemory::new(),
            requests: Requests::default(),
        };
        // Onto the Ui before frame 1, so palantir's own widgets paint right.
        ui.set_theme(app.theme.palantir.clone());
        // A remembered document that failed to load is forgotten, and that is
        // persisted, so the next launch does not fail on it again.
        if forgot_remembered {
            app.preferences.document_path = None;
            app.save_preferences();
        }
        app
    }

    /// Run a command the frame raised, once the pass that raised it is over:
    /// every one needs `&mut App`, a blocking dialog, or both.
    ///
    /// Returns whether it stranded the canvas's cached geometry. Only an edit
    /// can — the rest touch state no canvas measures against — but it is
    /// reported rather than requested here, so that `App::record` stays the one
    /// place in the app that asks for a relayout.
    #[must_use]
    fn run_command(&mut self, command: AppCommand) -> Relayout {
        match command {
            // Both replace the open document, so they clear the
            // unsaved-changes guard before doing anything.
            AppCommand::File(FileCommand::New) => self.guard_discard(PendingTransition::New),
            AppCommand::File(FileCommand::Open) => {
                self.guard_discard(PendingTransition::OpenPicked);
            }
            AppCommand::File(FileCommand::Save) => self.save_cur(),
            AppCommand::File(FileCommand::SaveAs) => self.save_document_as(DocumentFormat::Ron),
            AppCommand::File(FileCommand::SaveAsCompressed) => {
                self.save_document_as(DocumentFormat::ZippedRon);
            }
            AppCommand::Run(run) => self.run(run),
            AppCommand::Preferences(PreferencesCommand::Changed) => self.apply_preferences(),
            AppCommand::Preferences(PreferencesCommand::PickMlModel(kind)) => {
                self.pick_ml_model(kind);
            }
            AppCommand::Edit(EditCommand::PickInputPath(pick)) => {
                return self.pick_input_path(&pick);
            }
            AppCommand::View(ViewCommand::CopyImage) => {
                self.capture = Some(ViewCapture::new(CaptureTarget::Clipboard));
            }
            AppCommand::View(ViewCommand::SaveImage) => {
                if let Some(path) = self.dialogs.save_path(SavedFile::PNG, None) {
                    self.capture = Some(ViewCapture::new(CaptureTarget::Png(path)));
                }
            }
            AppCommand::Quit => self.guard_discard(PendingTransition::Quit),
        }
        Relayout::NotNeeded
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
                w.maximized = geom.placement.maximized;
                if !geom.placement.maximized {
                    w.size = geom.inner_size;
                    w.physical_pos = geom.placement.physical_pos;
                }
            }
            None => {
                self.preferences.window = Some(WindowState {
                    size: geom.inner_size,
                    maximized: geom.placement.maximized,
                    physical_pos: geom.placement.physical_pos,
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
        self.host.quit();
    }

    /// Whether a destructive transition has to prompt before proceeding:
    /// unsaved changes and the confirm preference both hold. The single
    /// predicate behind every path that replaces or discards the document.
    const fn needs_discard_confirmation(&self) -> bool {
        self.session.open.dirty && self.preferences.confirm_unsaved_changes
    }

    /// Carry out `transition`, or raise the unsaved-changes prompt first when
    /// the document holds edits worth protecting. Every path that would
    /// discard the open document routes through here — File ▸ New, File ▸
    /// Open, File ▸ Quit, ⌘Q — so the policy lives in one place instead of
    /// being restated (or forgotten) per caller.
    fn guard_discard(&mut self, transition: PendingTransition) {
        if self.needs_discard_confirmation() {
            self.discard.hold(transition);
        } else {
            self.perform(transition);
        }
    }

    /// Run a transition the guard cleared. `OpenPicked` picks its file here
    /// rather than before the prompt, so a cancelled prompt doesn't leave
    /// the user having chosen a file for nothing.
    fn perform(&mut self, transition: PendingTransition) {
        match transition {
            PendingTransition::Quit => self.quit(),
            PendingTransition::New => self.new_document(),
            PendingTransition::OpenPicked => self.load_picked_document(),
            // A failed load reports itself on the status line.
            PendingTransition::OpenAt(path) => {
                let _ = self.load_document(&path);
            }
        }
    }

    /// Open `path`, prompting first if the current document has unsaved
    /// edits. The entry point for a document the editor did not ask for —
    /// one the desktop hands over through
    /// [`crate::platform::route_opened_documents`].
    pub(crate) fn open_document_at(&mut self, path: PathBuf) {
        self.guard_discard(PendingTransition::OpenAt(path));
    }

    /// The titlebar X. The window stays open only while the prompt is up
    /// ([`DiscardGuard::record`] calls `keep_open`); with nothing to
    /// protect, the close proceeds untouched.
    fn handle_close_request(&mut self, ui: &Ui) {
        if !ui.close_requested() {
            return;
        }

        self.save_preferences();
        if self.needs_discard_confirmation() {
            self.discard.hold(PendingTransition::Quit);
        }
    }

    /// Apply the prompt's answer: run the save it asked for, then let
    /// [`DiscardOutcome::resolve`](crate::gui::app::discard_dialog::DiscardOutcome::resolve) decide — against the dirty flag that
    /// save left behind — whether the transition goes through and whether
    /// the guard stays on.
    fn apply_discard_outcome(&mut self, answered: Answered) {
        let Answered {
            transition,
            outcome,
        } = answered;
        if outcome.choice == DiscardChoice::Save {
            self.save_cur();
        }
        let resolution = outcome.resolve(self.session.open.dirty);
        if resolution.silence_prompt {
            self.set_confirm_unsaved(false);
        }
        if resolution.proceed {
            self.perform(transition);
        }
    }

    fn record_discard_prompt(&mut self, ui: &mut Ui) {
        let file_name = self
            .session
            .open
            .path
            .as_deref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str());
        if let Some(answered) = self.discard.record(ui, file_name) {
            self.apply_discard_outcome(answered);
        }
    }

    /// Open a file dialog for a node's `FsPath` const input and, if the
    /// user makes a selection, apply the chosen paths as a `SetInput` edit. Runs after
    /// authoring, so it goes through `OpenDocument::apply_edit` rather than
    /// the frame's intent drain.
    ///
    /// Reports the edit's relayout need rather than acting on it — this runs
    /// after `Session::frame` has handed its own back, and `App::record`
    /// spends both together.
    #[must_use]
    fn pick_input_path(&mut self, pick: &PathPick) -> Relayout {
        let extensions: Vec<&str> = pick.config.extensions.iter().map(String::as_str).collect();
        let picked = match pick.config.mode {
            FsPathMode::ExistingFile => self.dialogs.existing_file(&extensions).map(fs_path),
            FsPathMode::ExistingFiles => self.dialogs.existing_files(&extensions).map(fs_paths),
            FsPathMode::NewFile => self.dialogs.new_file(&extensions).map(fs_path),
            FsPathMode::Dir => self.dialogs.dir().map(fs_path),
        };
        let value = match picked {
            None => return Relayout::NotNeeded,
            Some(Ok(value)) => value,
            Some(Err(error)) => {
                self.status.error(StatusFamily::Edit, error.to_string());
                return Relayout::NotNeeded;
            }
        };
        self.status.succeeded(StatusFamily::Edit);
        Relayout::after(self.session.open.apply_edit(
            GraphIntent::SetInput {
                input: pick.port,
                to: Some(Binding::Const(value)),
            },
            self.runtime.library.cur(),
        ))
    }

    /// Prompt for a project file and load it. The
    /// [`PendingTransition::OpenPicked`] body: the picker runs here, *after*
    /// the guard cleared, so cancelling the unsaved-changes prompt never costs
    /// the user a file choice.
    fn load_picked_document(&mut self) {
        if let Some(path) = self
            .dialogs
            .project_open_path(self.session.open.path.as_deref())
        {
            let _ = self.load_document(&path);
        }
    }

    /// Replace the document with an empty one.
    fn new_document(&mut self) {
        self.adopt_document(OpenDocument::default());
    }

    /// Swap in `open` and reset every piece of state derived from the
    /// document it replaces.
    ///
    /// A fresh [`Session`] covers what it owns in one move: empty undo history
    /// (restoring the old document via Cmd-Z would replay intents that no longer
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
    ///
    /// A document failure on the status bar was about the document being
    /// replaced, so every adoption clears it — an empty document as much as a
    /// loaded one.
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
        self.status.succeeded(StatusFamily::Document);
    }

    /// Load `path` into a fresh editor, and say whether it loaded. A missing or corrupt file
    /// leaves the open document intact and surfaces its reason in the status bar.
    fn load_document(&mut self, path: &Path) -> bool {
        let Some(Loaded { open, report }) = OpenDocument::load_or_report(
            path.to_path_buf(),
            self.runtime.library.cur(),
            &mut self.status,
        ) else {
            return false;
        };
        self.adopt_document(open);
        report.report(&mut self.status);
        true
    }

    /// Cmd+S: overwrite the current file, in the format it was opened in, if
    /// there is one, else fall back to Save As (first save of a fresh document).
    fn save_cur(&mut self) {
        let format = self.session.open.format;
        match self.session.open.path.clone() {
            Some(path) => self.save_document(&path, format),
            None => self.save_document_as(format),
        }
    }

    /// Cmd+Shift+S / "Save As…" and "Save As Compressed…": always prompt for a destination,
    /// and write `format` there.
    fn save_document_as(&mut self, format: DocumentFormat) {
        if let Some(path) = self
            .dialogs
            .save_path(SavedFile::PROJECT, self.session.open.path.as_deref())
        {
            self.save_document(&path, format);
        }
    }

    /// Write the document to `path` in `format` and adopt both as the save target. Save-As
    /// moves the document, so the worker's disk cache repoints to the new
    /// location's store — the old one stays where it is.
    fn save_document(&mut self, path: &Path, format: DocumentFormat) {
        match self
            .session
            .open
            .save_to(path, self.preferences.saved_paths(), format)
        {
            Ok(()) => {
                self.runtime
                    .set_document_cache(self.session.open.path.as_deref());
                self.remember_document_path();
                self.status.succeeded(StatusFamily::Document);
            }
            Err(err) => self
                .status
                .error(StatusFamily::Document, format!("save failed: {err:#}")),
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
    fn apply_preferences(&mut self) {
        self.runtime.set_ml_model_paths(&self.preferences.ml_models);
        self.save_preferences();
    }

    /// Persist the preferences, surfacing a failed write in the status
    /// bar — the one save path every caller routes through, so a broken
    /// preferences file can't fail silently.
    fn save_preferences(&mut self) {
        match self.preferences_store.save(&self.preferences) {
            Ok(Saved::File) => self.status.succeeded(StatusFamily::Preferences),
            // Kept in the session: a failed load's report stays up.
            Ok(Saved::Memory) => {}
            Err(err) => self
                .status
                .error(StatusFamily::Preferences, err.to_string()),
        }
    }

    fn pick_ml_model(&mut self, kind: MlModelKind) {
        if let Some(path) = self.dialogs.existing_file(&["onnx"]) {
            self.set_ml_model_path(kind, path);
        }
    }

    fn set_ml_model_path(&mut self, kind: MlModelKind, path: PathBuf) {
        *kind.path_mut(&mut self.preferences.ml_models) = path;
        self.apply_preferences();
    }

    /// Persist whether discarding unsaved changes prompts to save.
    /// Shared by the Preferences checkbox (via `Changed`) and the prompt's
    /// "Don't ask again", which calls this directly.
    fn set_confirm_unsaved(&mut self, on: bool) {
        self.preferences.confirm_unsaved_changes = on;
        self.save_preferences();
    }

    /// Hand `command` to the runtime and report its outcome. A command that
    /// compiles supersedes the last run's failure; the ids were read frames
    /// ago, so a node may be gone by now, which the runtime answers as nothing
    /// to run.
    fn run(&mut self, command: RunCommand) {
        let graph = self.session.graph();
        let outcome = match command {
            RunCommand::Once => {
                self.runtime.run_once(graph);
                Ok(())
            }
            RunCommand::Node(node_id) => self.runtime.run_node(graph, node_id),
            // An eviction projects its outcome onto exactly the nodes it reaches.
            RunCommand::EvictCache(node_id) => self
                .runtime
                .evict_cache(graph, node_id)
                .map(|evicted| self.run_state.clear_cache_projections(&evicted)),
            // A flush only gains the value a copy on disk, so no projection moves.
            RunCommand::FlushCache(node_id) => self.runtime.flush_cache(graph, node_id),
            RunCommand::StartEvents => {
                self.runtime.start_event_loop(graph);
                Ok(())
            }
            RunCommand::Cancel => {
                self.runtime.cancel_run();
                return;
            }
            RunCommand::StopEvents => {
                self.runtime.stop_event_loop();
                return;
            }
        };
        match outcome {
            Ok(()) => self.status.succeeded(StatusFamily::Run),
            Err(error) => self.status.error(StatusFamily::Run, error.to_string()),
        }
    }

    /// Hand the image that landed this frame to the capture that asked for it.
    fn deliver_capture(&mut self, ui: &Ui) {
        let Some(landed) = self.capture.as_ref().and_then(|capture| capture.landed(ui)) else {
            return;
        };
        let capture = self
            .capture
            .take()
            .expect("the capture its image landed for");
        match capture.deliver(landed, &ui.clipboard()) {
            Ok(()) => self.status.succeeded(StatusFamily::View),
            Err(error) => self.status.error(StatusFamily::View, error.to_string()),
        }
    }
}

/// One picked path as an `FsPath` input value.
fn fs_path(path: PathBuf) -> Result<ConstValue, NonUnicodePath> {
    unicode(path).map(ConstValue::FsPath)
}

/// Several picked paths as an `FsPaths` input value — refused whole when any
/// one of them is not Unicode, since a partial list is not what was picked.
fn fs_paths(paths: Vec<PathBuf>) -> Result<ConstValue, NonUnicodePath> {
    paths
        .into_iter()
        .map(unicode)
        .collect::<Result<_, _>>()
        .map(ConstValue::FsPaths)
}

fn unicode(path: PathBuf) -> Result<String, NonUnicodePath> {
    path.into_os_string()
        .into_string()
        .map_err(|path| NonUnicodePath(PathBuf::from(path)))
}

impl palantir::App for App {
    fn update(&mut self, _win: palantir::WindowToken, ui: &Ui) {
        // First, so a driver reads the frame it saw, before this one changes anything.
        self.serve_driven(ui);

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
        // A new cache gets a line here rather than a third call site.
        //
        // A node deleted later in the same frame's record is swept next frame.
        // Nothing reads a dead entry in between — the geometry is reached
        // through a `NodeCtx` that only resolves for live nodes, and
        // `draw_panels` skips a panel whose node is gone — so the lag costs
        // memory and nothing else.
        self.run_state.sync(
            RunFeed {
                runtime: &self.runtime,
                status: &mut self.status,
            },
            ui,
            &self.session.open.document,
        );
        self.session.reconcile_caches();

        self.deliver_capture(ui);
        self.handle_close_request(ui);
    }

    fn record(&mut self, _win: palantir::WindowToken, ui: &mut Ui) {
        self.driven.serve_record(ui);
        // A request that waits, as for a screenshot to land, needs the frames to go on.
        if self.driven.pending() {
            ui.request_repaint();
        }

        // While nodes are computing, keep repainting (~20 fps) so the running
        // node's live elapsed-so-far timer ticks — a single long node emits no
        // progress events between its start and finish.
        if self.run_state.activity().is_executing() {
            ui.request_repaint_after(Duration::from_millis(100));
        }

        // One library snapshot for this record pass (a cheap Arc clone).
        // A command that publishes below is visible to pass B or the next frame.
        let library = Arc::clone(self.runtime.library.cur());
        // The frame's read-only world, composed once here: everything below
        // derives its own context from this one rather than taking the refs
        // again.
        let ctx = AppCtx::new(&self.theme, &library, &self.run_state).with_status(StatusInputs {
            message: self.status.cur(),
            process_memory: self.process_memory.sample(ui.now()),
        });
        let mut needs_relayout = self.session.frame(
            ui,
            ctx,
            FrameWrites {
                preferences: &mut self.preferences,
                requests: &mut self.requests,
            },
        );
        // What the session left behind: every command the frame raised, in the
        // order it raised them — a keyboard chord and a click on the same
        // frame both land. Popped one at a time so the queue is not borrowed
        // while a command runs, which is what lets `App::run_command` take all
        // of `self`.
        while let Some(command) = self.requests.pop_app() {
            needs_relayout |= self.run_command(command);
        }
        if let Some(capture) = &mut self.capture {
            capture.ask(ui);
        }
        // The app's one relayout request, past both tiers: everything that can
        // strand `CanvasGeometry`'s cross-frame caches has reported by here,
        // and a single pass answers however many of them fired.
        if needs_relayout == Relayout::Needed {
            ui.request_relayout();
        }

        self.record_discard_prompt(ui);
        // Last, after everything the pass could start: the last pass of a frame leaves it.
        self.pending_work.store(
            self.run_state.previews().in_flight() || self.capturing(),
            Ordering::Release,
        );
    }
}

/// What the driver reads of the app between frames, and the dialogs it answers.
impl App {
    pub(crate) const fn open_document(&self) -> &OpenDocument {
        &self.session.open
    }

    pub(crate) const fn status(&self) -> &StatusLog {
        &self.status
    }

    pub(crate) const fn run_state(&self) -> &RunState {
        &self.run_state
    }

    /// The library this session resolves the document's funcs against.
    pub(crate) const fn library(&self) -> &Arc<Library> {
        self.runtime.library.cur()
    }

    /// What a driver on another thread asks of the next frames.
    pub(crate) const fn driven_mut(&mut self) -> &mut DrivenRequests {
        &mut self.driven
    }

    /// Open `path` for a driver, as File ▸ Open does, with what `unsaved` says for unsaved edits
    /// in place of the prompt. A file that does not load leaves the open document as it was.
    pub(crate) fn open_driven(
        &mut self,
        path: &Path,
        unsaved: UnsavedEdits,
    ) -> Result<(), DrivenOpenError> {
        debug_assert!(path.is_absolute(), "{}", path.display());
        self.check_unsaved(unsaved)?;
        // Read from the load, not the open path: a failed load of the open document's own file
        // leaves that path open.
        if self.load_document(path) {
            Ok(())
        } else {
            Err(DrivenOpenError::NotOpened {
                path: path.to_path_buf(),
                status: self.status.cur().map(|message| message.line.to_string()),
            })
        }
    }

    /// Start a new document for a driver, as File ▸ New does, with what `unsaved` says for
    /// unsaved edits.
    pub(crate) fn new_driven(&mut self, unsaved: UnsavedEdits) -> Result<(), DrivenOpenError> {
        self.check_unsaved(unsaved)?;
        self.new_document();
        Ok(())
    }

    fn check_unsaved(&self, unsaved: UnsavedEdits) -> Result<(), DrivenOpenError> {
        if unsaved == UnsavedEdits::Refuse && self.session.open.dirty {
            Err(DrivenOpenError::Unsaved {
                document: self.session.open.path.clone(),
            })
        } else {
            Ok(())
        }
    }

    /// The flag a frame raises while work it does not show yet goes on, for a driver of the
    /// window to read without a frame of its own.
    pub(crate) fn pending_work(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.pending_work)
    }

    /// Whether a View ▸ Copy or Save of the view waits for its image.
    pub(crate) const fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    pub(crate) const fn dialogs_mut(&mut self) -> &mut Dialogs {
        &mut self.dialogs
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::sync::Arc;

    use palantir::Ui;

    use crate::core::io::preferences::preferences_store::PreferencesStore;
    use crate::gui::app::app_host::AppHost;
    use crate::gui::app::offscreen_signals::OffscreenSignals;
    use crate::gui::app::{App, LaunchInputs};
    use crate::gui::dialogs::{DialogFallback, Dialogs};

    impl App {
        /// An app on a new document, its preferences in memory and its dialogs refused, for a
        /// test that drives it frame by frame.
        pub(crate) fn offscreen(ui: &mut Ui) -> Self {
            Self::new(
                ui,
                AppHost::Offscreen(Arc::new(OffscreenSignals::default())),
                LaunchInputs {
                    preferences: PreferencesStore::Memory.open(),
                    document: None,
                    dialogs: Dialogs::new(DialogFallback::Refuse),
                },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Unicode pick becomes the value as it is. One unit that is not Unicode
    /// refuses the whole pick and names that path, instead of replacing the
    /// unit and pointing the node at a file that does not exist.
    #[test]
    fn a_pick_that_is_not_unicode_is_refused_whole() {
        use common::internals::non_unicode_name;

        let good = PathBuf::from("/data/a.fit");
        let mut bad_name = non_unicode_name(0);
        bad_name.push("b.fit");
        let bad = Path::new("/data").join(bad_name);

        assert!(matches!(
            fs_path(good.clone()),
            Ok(ConstValue::FsPath(path)) if path == "/data/a.fit"
        ));
        assert!(matches!(
            fs_paths(vec![good.clone()]),
            Ok(ConstValue::FsPaths(paths)) if paths == ["/data/a.fit"]
        ));
        assert!(matches!(fs_path(bad.clone()), Err(NonUnicodePath(path)) if path == bad));
        assert!(matches!(
            fs_paths(vec![good, bad.clone()]),
            Err(NonUnicodePath(path)) if path == bad
        ));
    }

    /// A handed-in document waits on the prompt while the open one has unsaved edits, and the
    /// answer decides: Don't Save opens it, a Save whose dialog is cancelled leaves the edits and
    /// the document open, as Cancel does. Each answer takes the prompt down.
    #[test]
    fn a_handed_in_document_waits_on_the_prompt_and_its_answer() {
        use common::TempDir;
        use glam::UVec2;
        use palantir::internals::ui_harness::UiHarness;

        use crate::core::io::document::saved_paths::SavedPaths;
        use crate::gui::app::discard_dialog::DiscardOutcome;

        let dir = TempDir::new("darkroom-discard-prompt");
        let handed = dir.join("handed.darkroom");
        OpenDocument::default()
            .save_to(&handed, SavedPaths::Relative, DocumentFormat::Ron)
            .unwrap();
        for (choice, opened) in [
            (DiscardChoice::Discard, true),
            (DiscardChoice::Save, false),
            (DiscardChoice::Cancel, false),
        ] {
            let mut ui = UiHarness::with_text(UVec2::new(800, 600));
            let mut app = App::offscreen(ui.ui());
            app.session.open.dirty = true;
            app.open_document_at(handed.clone());
            assert!(app.discard.is_up(), "{choice:?}: the prompt holds the open");
            assert_eq!(app.session.open.path, None);

            let answered = app
                .discard
                .answer(DiscardOutcome {
                    choice,
                    dont_ask_again: false,
                })
                .unwrap();
            app.apply_discard_outcome(answered);
            assert!(!app.discard.is_up(), "{choice:?}: the answer takes it down");
            assert_eq!(
                (app.session.open.path.as_ref(), app.session.open.dirty),
                if opened {
                    (Some(&handed), false)
                } else {
                    (None, true)
                },
                "{choice:?}"
            );
        }
    }

    /// A frame leaves the pending-work flag raised while a view capture waits for its image,
    /// which the deviceless harness never paints, and lowered once nothing waits.
    #[test]
    fn a_frame_publishes_the_work_it_leaves_pending() {
        use glam::UVec2;
        use palantir::internals::ui_harness::UiHarness;

        use crate::gui::frame::app_command::ViewCommand;

        let mut ui = UiHarness::with_text(UVec2::new(800, 600));
        let mut app = App::offscreen(ui.ui());
        let pending = app.pending_work();
        ui.frame_app(&mut app);
        assert!(!pending.load(Ordering::Acquire), "nothing waits");
        let _ = app.run_command(AppCommand::View(ViewCommand::CopyImage));
        ui.frame_app(&mut app);
        assert!(pending.load(Ordering::Acquire), "the capture waits");
        app.capture = None;
        ui.frame_app(&mut app);
        assert!(!pending.load(Ordering::Acquire));
    }

    /// A driver's open loads the file and reports a load that fails, also of the open document's
    /// own file, which stays open; with unsaved edits it refuses, and a new document too, unless
    /// told to discard them.
    #[test]
    fn a_driven_open_reports_its_load_and_keeps_unsaved_edits() {
        use common::{TempDir, fs};
        use glam::UVec2;
        use palantir::internals::ui_harness::UiHarness;

        use crate::core::io::document::saved_paths::SavedPaths;

        let dir = TempDir::new("darkroom-driven-open");
        let document = dir.join("scene.darkroom");
        OpenDocument::default()
            .save_to(&document, SavedPaths::Relative, DocumentFormat::Ron)
            .unwrap();
        let mut ui = UiHarness::with_text(UVec2::new(800, 600));
        let mut app = App::offscreen(ui.ui());
        app.open_driven(&document, UnsavedEdits::Refuse).unwrap();
        assert_eq!(app.session.open.path.as_ref(), Some(&document));

        fs::write(&document, b"not a document").unwrap();
        match app.open_driven(&document, UnsavedEdits::Refuse) {
            Err(DrivenOpenError::NotOpened { path, status }) => {
                assert_eq!(path, document);
                assert!(status.is_some_and(|line| line.starts_with("load failed: ")));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            app.session.open.path.as_ref(),
            Some(&document),
            "still open"
        );

        app.session.open.dirty = true;
        let unsaved = DrivenOpenError::Unsaved {
            document: Some(document.clone()),
        };
        assert_eq!(
            app.open_driven(&document, UnsavedEdits::Refuse),
            Err(unsaved.clone())
        );
        assert_eq!(app.new_driven(UnsavedEdits::Refuse), Err(unsaved));
        assert!(app.session.open.dirty, "the edits stay");
        app.new_driven(UnsavedEdits::Discard).unwrap();
        assert_eq!(app.session.open.path, None, "a discard goes through");
        assert!(!app.session.open.dirty);
    }

    /// An app on the driver's host: a scripted Open dialog opens the answered document; with a
    /// file store the open is remembered in that file, and with the memory store nothing is
    /// written. A file store whose load failed runs as the memory store, so the file stays as the
    /// user has it, and the status line says so.
    #[test]
    fn a_scripted_open_loads_the_answer_and_the_store_decides_what_persists() {
        use common::{TempDir, fs};
        use glam::UVec2;
        use palantir::internals::ui_harness::UiHarness;

        use crate::core::io::document::saved_paths::SavedPaths;
        use crate::gui::app::offscreen_signals::OffscreenSignals;
        use crate::gui::dialogs::DialogAnswer;
        use crate::gui::dialogs::DialogFallback;

        let dir = TempDir::new("darkroom-scripted-open");
        let document = dir.join("scene.darkroom");
        OpenDocument::default()
            .save_to(&document, SavedPaths::Relative, DocumentFormat::Ron)
            .unwrap();
        let preferences_file = dir.join("darkroom.preferences.ron");

        // A file store over a valid file, a memory store, and a file store over
        // a file that does not parse, which opens as a memory store.
        for (store, readable) in [
            (PreferencesStore::File(preferences_file.clone()), true),
            (PreferencesStore::Memory, true),
            (PreferencesStore::File(preferences_file.clone()), false),
        ] {
            fs::remove_file(&preferences_file).ok();
            if !readable {
                fs::write(&preferences_file, b"not ron").unwrap();
            }
            let mut ui = UiHarness::with_text(UVec2::new(800, 600));
            let mut dialogs = Dialogs::new(DialogFallback::Refuse);
            dialogs.queue(DialogAnswer::Path(document.clone()));
            let mut app = App::new(
                ui.ui(),
                AppHost::Offscreen(Arc::new(OffscreenSignals::default())),
                LaunchInputs {
                    preferences: store.clone().open(),
                    document: None,
                    dialogs,
                },
            );
            assert_eq!(app.session.open.path, None, "{store:?}: launched empty");
            let _ = app.run_command(AppCommand::File(FileCommand::Open));
            assert_eq!(app.session.open.path.as_ref(), Some(&document), "{store:?}");
            if !readable {
                assert_eq!(app.preferences_store, PreferencesStore::Memory);
                assert!(
                    app.status
                        .cur()
                        .is_some_and(|message| message.line.contains("settings are not saved")),
                    "the status line says the settings are not saved: {:?}",
                    app.status.cur()
                );
            }
            match store {
                PreferencesStore::File(_) if readable => assert_eq!(
                    store.clone().open().preferences.document_path,
                    Some(document.clone()),
                    "the file store remembers the open"
                ),
                PreferencesStore::File(_) => assert_eq!(
                    fs::read(&preferences_file).unwrap(),
                    b"not ron",
                    "the file that did not load stays as it was"
                ),
                PreferencesStore::Memory => assert!(
                    !fs::exists(&preferences_file),
                    "the memory store writes nothing"
                ),
            }
        }
    }
}
