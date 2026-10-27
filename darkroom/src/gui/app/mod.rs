use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use palantir::Ui;
use scenarium::Binding;
use scenarium::ConstValue;
use scenarium::FsPathMode;
use scenarium::Library;
use scenarium::NodeId;

use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::relayout::Relayout;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::preferences::error::PreferencesLoadError;
use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::core::io::preferences::{Preferences, WindowState};
use crate::core::open_document::{LaunchSource, OpenDocument};
use crate::core::runtime_host::RuntimeHost;
use crate::core::status_log::{StatusFamily, StatusLog};
use crate::driver::error::DriverError;
use crate::gui::app::app_host::AppHost;
use crate::gui::app::discard_dialog::{DiscardChoice, DiscardOutcome};
use crate::gui::app::driven_requests::DrivenRequests;
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
use crate::gui::frame::requests::Requests;
use crate::gui::state::process_memory::ProcessMemory;
use crate::gui::state::run_state::{RunFeed, RunState};
use crate::gui::theme::Theme;

pub(crate) mod app_host;
mod discard_dialog;
pub(crate) mod driven_requests;
mod error;
pub(crate) mod offscreen_signals;
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
    /// The document-replacing transition waiting on the unsaved-changes
    /// prompt, and thus whether that prompt is up at all. Raised by
    /// [`Self::guard_discard`]; cleared when the user answers.
    confirm_discard: Option<PendingTransition>,
    /// The image of the window a View command waits on; a second command replaces the first.
    capture: Option<ViewCapture>,
    /// What a driver on another thread asked of the next frames.
    driven: DrivenRequests,
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
    OpenAt(PathBuf),
}

impl PendingTransition {
    /// How the prompt finishes "Save changes to X before …?".
    const fn prompt_tail(&self) -> &'static str {
        match self {
            Self::Quit => "quitting",
            Self::New => "closing it",
            Self::OpenPicked | Self::OpenAt(_) => "opening another document",
        }
    }
}

/// What the app is launched with, read before its window exists.
#[derive(Debug)]
pub(crate) struct LaunchInputs {
    /// The preferences as `store` loaded them; a failure leaves the session on defaults it
    /// does not save.
    pub(crate) preferences: Result<Preferences, PreferencesLoadError>,
    /// Where the preferences came from, and where they persist.
    pub(crate) store: PreferencesStore,
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
            preferences,
            store,
            document,
            dialogs,
        } = launch;
        let wake = host.wake();
        // `preferences` is loaded in `run` before the window exists, so
        // its saved geometry can size the window at creation.
        let mut status = StatusLog::default();
        let (mut preferences, preferences_store) = match preferences {
            Ok(preferences) => (preferences, store),
            Err(error) => {
                status.error(
                    StatusFamily::Preferences,
                    format!("{error}; this session's settings are not saved"),
                );
                (Preferences::default(), PreferencesStore::Memory)
            }
        };
        let run_state = RunState::new(Arc::clone(&wake));
        let mut runtime = RuntimeHost::new(wake, &preferences);
        let remembered = preferences.document_path.clone();
        let open = OpenDocument::open_at_launch(
            LaunchSource {
                argument: document,
                preferences: &mut preferences,
            },
            &mut status,
            runtime.library.cur(),
        );
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
            confirm_discard: None,
            capture: None,
            driven: DrivenRequests::default(),
            process_memory: ProcessMemory::new(),
            requests: Requests::default(),
        };
        // Onto the Ui before frame 1, so palantir's own widgets paint right.
        ui.set_theme(app.theme.palantir.clone());
        // A remembered document that failed to load was forgotten; persist
        // that, so the next launch does not fail on it again.
        if app.preferences.document_path != remembered {
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
            AppCommand::Run(RunCommand::Once) => self.run_graph(),
            AppCommand::Run(RunCommand::Node(node_id)) => self.run_node(node_id),
            AppCommand::Run(RunCommand::EvictCache(node_id)) => self.evict_cache(node_id),
            AppCommand::Run(RunCommand::FlushCache(node_id)) => self.flush_cache(node_id),
            AppCommand::Run(RunCommand::Cancel) => self.runtime.cancel_run(),
            AppCommand::Run(RunCommand::StartEvents) => self.start_events(),
            AppCommand::Run(RunCommand::StopEvents) => self.stop_events(),
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
                    w.pos = geom.placement.pos;
                }
            }
            None => {
                self.preferences.window = Some(WindowState {
                    size: geom.inner_size,
                    maximized: geom.placement.maximized,
                    pos: geom.placement.pos,
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
            self.confirm_discard = Some(transition);
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
            self.save_cur();
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
        self.session.open.apply_edit(
            GraphIntent::SetInput {
                input: pick.port,
                to: Some(Binding::Const(value)),
            },
            self.runtime.library.cur(),
        )
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
        let loaded = match OpenDocument::load(path.to_path_buf(), self.runtime.library.cur()) {
            Ok(loaded) => loaded,
            Err(err) => {
                self.status
                    .error(StatusFamily::Document, format!("load failed: {err:#}"));
                return false;
            }
        };
        self.adopt_document(loaded.open);
        loaded.report.report(&mut self.status);
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
        self.runtime.configure_ml_model_defaults(&self.preferences);
        self.save_preferences();
    }

    /// Persist the preferences, surfacing a failed write in the status
    /// bar — the one save path every caller routes through, so a broken
    /// preferences file can't fail silently.
    fn save_preferences(&mut self) {
        let PreferencesStore::File(path) = &self.preferences_store else {
            return;
        };
        match self.preferences.save_to(path) {
            Ok(()) => self.status.succeeded(StatusFamily::Preferences),
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

    /// Compile the document graph and execute its sinks once on the
    /// worker. Worker status reports acknowledge actual execution and
    /// event-loop transitions.
    fn run_graph(&mut self) {
        self.runtime
            .run_once(self.session.graph(), &mut self.status);
    }

    /// Like [`Self::run_graph`], but seeds the run at one node: only its
    /// upstream cone executes and its outputs are delivered.
    ///
    /// The id was read frames ago, so the node may be gone by now; the
    /// runtime reports that as nothing to run.
    fn run_node(&mut self, node_id: NodeId) {
        self.runtime
            .run_node(self.session.graph(), node_id, &mut self.status);
    }

    /// Evict one node's cache cone and project the outcome onto exactly the
    /// nodes it reaches. An empty answer means the program holds no work for
    /// the node, so there is nothing to project either.
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
    /// fire their subscribers until stopped.
    fn start_events(&mut self) {
        self.runtime
            .start_event_loop(self.session.graph(), &mut self.status);
    }

    /// Stop the worker's event loop.
    fn stop_events(&self) {
        self.runtime.stop_event_loop();
    }

    /// Hand the image that landed this frame to the capture that asked for it.
    fn deliver_capture(&mut self, ui: &Ui) {
        let Some(shot) = ui.screenshot_taken() else {
            return;
        };
        let Some(capture) = self.capture.take_if(|capture| capture.asked()) else {
            return;
        };
        match capture.deliver(shot, &ui.clipboard()) {
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
        let mut driven = self.driven.lend();
        driven.serve(self, ui);
        self.driven.restore(driven);

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
        if self.run_state.activity.is_executing() {
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
            process_memory: self.process_memory.sample(Instant::now()),
        });
        // Reset here rather than inside the frame: the queue is `App`'s, and
        // `record` runs twice on a frame carrying action input — the second
        // pass must not inherit what the first already ran.
        self.requests.clear();
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

    /// Open `path` for a driver, as File ▸ Open does but refusing to discard unsaved edits,
    /// which a person in the window may own: the driver says so instead of raising the prompt.
    pub(crate) fn open_driven(&mut self, path: &Path) -> Result<(), DriverError> {
        debug_assert!(path.is_absolute(), "{}", path.display());
        self.refuse_unsaved()?;
        // Read from the load, not the open path: a failed load of the open document's own file
        // leaves that path open.
        if self.load_document(path) {
            Ok(())
        } else {
            Err(DriverError::NotOpened {
                path: path.to_path_buf(),
                status: self.status.cur().map(|message| message.line.to_string()),
            })
        }
    }

    /// Start a new document for a driver, as File ▸ New does but refusing to discard unsaved
    /// edits.
    pub(crate) fn new_driven(&mut self) -> Result<(), DriverError> {
        self.refuse_unsaved()?;
        self.new_document();
        Ok(())
    }

    fn refuse_unsaved(&self) -> Result<(), DriverError> {
        if self.session.open.dirty {
            Err(DriverError::Unsaved {
                document: self.session.open.path.clone(),
            })
        } else {
            Ok(())
        }
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

    /// A driver's open loads the file and reports a load that fails, also of the open document's
    /// own file, which stays open; with unsaved edits it refuses, and a new document too, rather
    /// than discard them.
    #[test]
    fn a_driven_open_reports_its_load_and_keeps_unsaved_edits() {
        use common::{TempDir, fs};
        use glam::UVec2;
        use palantir::internals::ui_harness::UiHarness;

        use crate::core::io::document::saved_paths::SavedPaths;
        use crate::gui::app::offscreen_signals::OffscreenSignals;
        use crate::gui::dialogs::DialogFallback;

        let dir = TempDir::new("darkroom-driven-open");
        let document = dir.join("scene.darkroom");
        OpenDocument::default()
            .save_to(&document, SavedPaths::Relative, DocumentFormat::Ron)
            .unwrap();
        let mut ui = UiHarness::with_text(UVec2::new(800, 600));
        let mut app = App::new(
            ui.ui(),
            AppHost::Offscreen(Arc::new(OffscreenSignals::default())),
            LaunchInputs {
                preferences: Ok(Preferences::default()),
                store: PreferencesStore::Memory,
                document: None,
                dialogs: Dialogs::new(DialogFallback::Refuse),
            },
        );
        app.open_driven(&document).unwrap();
        assert_eq!(app.session.open.path.as_ref(), Some(&document));

        fs::write(&document, b"not a document").unwrap();
        match app.open_driven(&document) {
            Err(DriverError::NotOpened { path, status }) => {
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
        for refused in [app.open_driven(&document), app.new_driven()] {
            match refused {
                Err(DriverError::Unsaved { document: open }) => {
                    assert_eq!(open.as_ref(), Some(&document));
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(app.session.open.dirty, "the edits stay");
    }

    /// An app on the driver's host: a scripted Open dialog opens the answered document; with a
    /// file store the open is remembered in that file, and with the memory store nothing is
    /// written. A file store whose load failed runs as the memory store, so the file stays as the
    /// user has it, and the status line says so.
    #[test]
    fn a_scripted_open_loads_the_answer_and_the_store_decides_what_persists() {
        use std::io;

        use common::{TempDir, fs};
        use glam::UVec2;
        use palantir::internals::ui_harness::UiHarness;

        use crate::core::io::document::saved_paths::SavedPaths;
        use crate::driver::action::DialogAnswer;
        use crate::gui::app::offscreen_signals::OffscreenSignals;
        use crate::gui::dialogs::DialogFallback;

        let dir = TempDir::new("darkroom-scripted-open");
        let document = dir.join("scene.darkroom");
        OpenDocument::default()
            .save_to(&document, SavedPaths::Relative, DocumentFormat::Ron)
            .unwrap();
        let preferences_file = dir.join("darkroom.preferences.ron");

        let unreadable = || PreferencesLoadError::Read {
            path: preferences_file.clone(),
            source: io::Error::other("unreadable"),
        };
        for (store, preferences) in [
            (
                PreferencesStore::File(preferences_file.clone()),
                Ok(Preferences::default()),
            ),
            (PreferencesStore::Memory, Ok(Preferences::default())),
            (
                PreferencesStore::File(preferences_file.clone()),
                Err(unreadable()),
            ),
        ] {
            let failed = preferences.is_err();
            if store == PreferencesStore::Memory || failed {
                // Left by the file store's round; gone, so any write would show.
                fs::remove_file(&preferences_file).ok();
            }
            let mut ui = UiHarness::with_text(UVec2::new(800, 600));
            let mut dialogs = Dialogs::new(DialogFallback::Refuse);
            dialogs.queue(DialogAnswer::Path(document.clone()));
            let mut app = App::new(
                ui.ui(),
                AppHost::Offscreen(Arc::new(OffscreenSignals::default())),
                LaunchInputs {
                    preferences,
                    store: store.clone(),
                    document: None,
                    dialogs,
                },
            );
            assert_eq!(app.session.open.path, None, "{store:?}: launched empty");
            let _ = app.run_command(AppCommand::File(FileCommand::Open));
            assert_eq!(app.session.open.path.as_ref(), Some(&document), "{store:?}");
            if failed {
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
                PreferencesStore::File(_) if !failed => assert_eq!(
                    store.load().unwrap().document_path,
                    Some(document.clone()),
                    "the file store remembers the open"
                ),
                _ => assert!(
                    !fs::exists(&preferences_file),
                    "{store:?}, failed {failed}: nothing is written"
                ),
            }
        }
    }
}
