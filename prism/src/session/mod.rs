use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use common::SerdeFormat;
use palantir::ImageFuncLib;
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::WorkerEventsFuncLib;
use scenarium::execution_graph::{self, ArgumentValues};
use scenarium::graph::NodeId;
use scenarium::prelude::{ExecutionStats, FuncLib};
use scenarium::testing::{TestFuncHooks, test_func_lib};
use scenarium::worker::{Worker, WorkerMessage};
use tokio::sync::oneshot;

use crate::config::Config;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::launch_config::LaunchConfig;
use crate::model::argument_values_cache::{CacheEvent, NodeCache, RenderEvent, invalidated_nodes};
use crate::model::intent;
use crate::model::{ActionStack, Intent, ViewGraph};
use crate::script::{self, ScriptExecutor, SessionInbound};
use crate::session::output::{EditorCommand, FrameOutput, RunCommand};
use crate::ui_host::UiHost;

pub mod output;

#[cfg(test)]
mod tests;

const UNDO_MAX_STEPS: usize = 256;

/// Status buffer size cap (UI-visible log).
const STATUS_CAP: usize = 2000;

/// Everything the worker-side plumbing pushes back into the session.
/// One enum, one channel — `drain_inbound` drains it in a single loop.
/// New worker→session signals add a variant, not a field.
#[derive(Debug)]
enum WorkerEvent {
    ExecutionFinished(Result<ExecutionStats, execution_graph::Error>),
    ArgumentValues {
        node_id: NodeId,
        values: Option<ArgumentValues>,
    },
    Print(String),
    /// User picked a path in the native save dialog (None = cancelled).
    SavePicked(Option<PathBuf>),
    /// User picked a path in the native open dialog (None = cancelled).
    LoadPicked(Option<PathBuf>),
}

/// Frontend-agnostic editor core. Each frame, frontends must call
/// `drain_inbound()` *before* reading [`Session::graph_context`] (so
/// the view sees freshly-arrived worker results) and `handle_output()`
/// *after* rendering (so queued actions and run commands are applied).
/// `exit()` runs once on shutdown.
#[derive(Debug)]
pub struct Session {
    func_lib: Arc<FuncLib>,
    view_graph: ViewGraph,
    execution_stats: Option<ExecutionStats>,
    /// Session→renderer signals queued for `GraphUi::render` to drain.
    /// Carries cache mutations (texture state lives on `GraphUi`) and
    /// the `Reset` signal pushed on graph swap so the renderer drops
    /// per-graph state (gesture, popups, layout galleys) atomically.
    render_events: Vec<RenderEvent>,
    status: String,
    config: Config,

    graph_dirty: bool,
    action_stack: ActionStack,

    worker: Option<Worker>,
    worker_tx: UnboundedSender<WorkerEvent>,
    worker_rx: UnboundedReceiver<WorkerEvent>,

    script_executor: Option<ScriptExecutor>,
    script_inbound_rx: UnboundedReceiver<SessionInbound>,
    /// Run-state command queued by a `SessionInbound::Run*` (i.e. a
    /// script). Consumed by `handle_output`, which already owns the
    /// "dirty → Update → ExecuteTerminals" sequencing — having scripts
    /// route through it ensures the worker sees the latest graph
    /// before evaluating. GUI's `output.run_cmd()` still wins on
    /// conflict so direct user input takes precedence over background
    /// scripting.
    pending_run_cmd: Option<RunCommand>,

    ui_host: Arc<dyn UiHost>,
}

impl Session {
    pub fn new<H: UiHost + 'static>(ui_host: H, launch_config: LaunchConfig) -> Self {
        let ui_host: Arc<dyn UiHost> = Arc::new(ui_host);
        let (worker_tx, worker_rx) = unbounded_channel::<WorkerEvent>();

        let worker = Worker::new({
            let ui = ui_host.clone();
            let tx = worker_tx.clone();
            move |result| {
                let _ = tx.send(WorkerEvent::ExecutionFinished(result));
                ui.request_redraw();
            }
        });

        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(sample_test_hooks(worker_tx.clone())));
        func_lib.merge(BasicFuncLib::default());
        func_lib.merge(WorkerEventsFuncLib::default());
        func_lib.merge(ImageFuncLib::default());
        let func_lib = Arc::new(func_lib);

        let (script_inbound_tx, script_inbound_rx) = unbounded_channel::<SessionInbound>();
        // Wake the host loop after every script side-effect so the
        // next `drain_inbound` runs promptly. Opaque `Fn()` keeps the
        // script crate frontend-agnostic.
        let notify: script::Notify = {
            let ui_host = ui_host.clone();
            Arc::new(move || ui_host.request_redraw())
        };
        let script_cfg = script::ScriptConfig {
            tcp: launch_config.actual.script_tcp.clone(),
        };
        let script_executor = ScriptExecutor::new(
            script::start_transports(&script_cfg),
            script_inbound_tx,
            func_lib.clone(),
            notify,
        );

        let mut result = Self::from_parts(
            func_lib,
            launch_config.saved,
            Some(worker),
            worker_tx,
            worker_rx,
            Some(script_executor),
            script_inbound_rx,
            ui_host,
        );

        if launch_config.actual.load_last
            && let Some(path) = result.config.current_path.clone()
        {
            result.load_graph(&path);
        }

        result
    }

    /// Single struct-literal site shared by `new` and the test stub.
    /// Adding a `Session` field touches one spot — the test stub can't
    /// silently desync.
    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        func_lib: Arc<FuncLib>,
        config: Config,
        worker: Option<Worker>,
        worker_tx: UnboundedSender<WorkerEvent>,
        worker_rx: UnboundedReceiver<WorkerEvent>,
        script_executor: Option<ScriptExecutor>,
        script_inbound_rx: UnboundedReceiver<SessionInbound>,
        ui_host: Arc<dyn UiHost>,
    ) -> Self {
        Self {
            func_lib,
            view_graph: ViewGraph::default(),
            execution_stats: None,
            render_events: Vec::new(),
            status: String::new(),
            config,
            graph_dirty: true,
            action_stack: ActionStack::new(UNDO_MAX_STEPS),
            worker,
            worker_tx,
            worker_rx,
            script_inbound_rx,
            pending_run_cmd: None,
            ui_host,
            script_executor,
        }
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    /// Derived from the worker's event-loop state, which is the
    /// canonical owner. No cached field — avoids start/stop races
    /// between `handle_output` toggling and the worker actually acking.
    pub fn autorun(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(Worker::is_event_loop_started)
    }

    /// Frame-level dependency bundle for the view layer. Fully
    /// shared-borrowed: every mutation goes through
    /// `Intent::apply` in `commit_actions`. The
    /// `ArgumentValuesCache` lives on the renderer; cache updates
    /// flow through [`Session::take_render_events`].
    pub fn graph_context(&self) -> GraphContext<'_> {
        let execution_stats = self.execution_stats.as_ref();
        GraphContext {
            func_lib: &self.func_lib,
            view_graph: &self.view_graph,
            execution_stats,
            exec_info_index: crate::model::NodeExecutionIndex::new(execution_stats),
            autorun: self.autorun(),
        }
    }

    /// Drain pending Session→renderer signals (cache mutations and
    /// graph-swap resets). `GraphUi::render` applies them at frame start.
    pub fn take_render_events(&mut self) -> Vec<RenderEvent> {
        std::mem::take(&mut self.render_events)
    }

    /// Drive Session for one tick: drain script + worker inbounds,
    /// then dispatch the supplied `output` (worker commands, queued
    /// runs, edited intents, etc.) Used by non-GUI frontends
    /// (headless, TUI) which have no render pass to interleave with
    /// drain/dispatch. The GUI doesn't call this — it splits the two
    /// halves across `App::logic` and `App::ui` so script side-effects
    /// flow even when the window is hidden.
    ///
    /// Caller owns `output`. For non-render frontends just pass a
    /// `FrameOutput::default()` field; it stays empty, but
    /// `handle_output` still needs to see it to consume
    /// `pending_run_cmd` queued by `drain_inbound`.
    pub fn tick(&mut self, output: &mut FrameOutput) {
        self.drain_inbound();
        self.handle_output(output);
    }

    /// Appends a status line. Keeps the buffer below [`STATUS_CAP`]
    /// by draining oldest content.
    pub fn add_status(&mut self, message: impl AsRef<str>) {
        if !self.status.is_empty() {
            self.status.push('\n');
        }
        self.status.push_str(message.as_ref());
        if self.status.len() > STATUS_CAP {
            self.status.drain(..self.status.len() - STATUS_CAP);
        }
    }

    /// Apply intents to `view_graph` in order, returning `true` if any
    /// affects computation. Does *not* record undo history — that's
    /// the job of [`Session::commit_actions`]. Currently used only by
    /// tests; production code commits through
    /// [`Session::commit_intents`].
    #[cfg(test)]
    pub fn apply(&mut self, intents: &[Intent]) -> bool {
        let mut graph_updated = false;
        for intent in intents {
            let step = crate::model::intent::build_step(intent.clone(), &self.view_graph);
            crate::model::intent::apply_step(&step, &mut self.view_graph);
            graph_updated |= intent::affects_computation(&step);
        }
        graph_updated
    }

    pub fn empty_graph(&mut self) {
        self.replace_graph(ViewGraph::default());
        self.add_status("Created new graph");
    }

    /// Save to the current path if one is set, otherwise open the
    /// async save dialog. Result lands via `WorkerEvent::SavePicked`.
    pub fn save_graph_dialog(&self) {
        if let Some(path) = self.config.current_path.clone() {
            // Synchronous save via a posted-back event keeps a single
            // code path for "graph just got saved".
            let tx = self.worker_tx.clone();
            let ui = self.ui_host.clone();
            tokio::spawn(async move {
                let _ = tx.send(WorkerEvent::SavePicked(Some(path)));
                ui.request_redraw();
            });
        } else {
            self.save_graph_as_dialog();
        }
    }

    /// Always open the async save dialog. Result lands via
    /// `WorkerEvent::SavePicked`.
    pub fn save_graph_as_dialog(&self) {
        let tx = self.worker_tx.clone();
        let ui = self.ui_host.clone();
        tokio::spawn(async move {
            let handle = rfd::AsyncFileDialog::new()
                .add_filter("Rhai", &["rhai"])
                .add_filter("JSON", &["json"])
                .add_filter("Lz4 compressed Rhai", &["lz4"])
                .save_file()
                .await;
            let path = handle.map(|h| h.path().to_path_buf());
            let _ = tx.send(WorkerEvent::SavePicked(path));
            ui.request_redraw();
        });
    }

    /// Open the async load dialog. Result lands via
    /// `WorkerEvent::LoadPicked`.
    pub fn load_graph_dialog(&self) {
        let tx = self.worker_tx.clone();
        let ui = self.ui_host.clone();
        tokio::spawn(async move {
            let handle = rfd::AsyncFileDialog::new()
                .add_filter("All supported", &["rhai", "json", "lz4"])
                .add_filter("Rhai", &["rhai"])
                .add_filter("JSON", &["json"])
                .add_filter("Lz4 compressed Rhai", &["lz4"])
                .pick_file()
                .await;
            let path = handle.map(|h| h.path().to_path_buf());
            let _ = tx.send(WorkerEvent::LoadPicked(path));
            ui.request_redraw();
        });
    }

    pub fn save_graph(&mut self, path: &Path) {
        match self.write_graph_to(path) {
            Ok(()) => {
                self.config.current_path = Some(path.to_path_buf());
                self.add_status(format!("Saved graph to {}", path.display()));
            }
            Err(err) => self.add_status(format!("Save failed: {} {err}", path.display())),
        }
    }

    pub fn load_graph(&mut self, path: &Path) {
        match self.read_graph_from(path) {
            Ok(()) => {
                self.config.current_path = Some(path.to_path_buf());
                self.add_status(format!("Loaded graph from {}", path.display()));
            }
            Err(err) => {
                self.config.current_path = None;
                self.add_status(format!("Load failed: {} {err}", path.display()));
            }
        }
    }

    fn write_graph_to(&self, path: &Path) -> Result<()> {
        let format = SerdeFormat::from_file_name(path.to_string_lossy().as_ref())?;
        let payload = self.view_graph.serialize(format);
        std::fs::write(path, payload).map_err(anyhow::Error::from)
    }

    fn read_graph_from(&mut self, path: &Path) -> Result<()> {
        let format = SerdeFormat::from_file_name(path.to_string_lossy().as_ref())?;
        let payload = std::fs::read(path)?;
        self.replace_graph(ViewGraph::deserialize(format, &payload)?);
        Ok(())
    }

    /// Drains worker→session and script→session channels into Session
    /// state (status log, execution stats, argument-value cache).
    /// Frontends call this on every host tick before reading graph
    /// state (GUI: `App::logic`; headless / TUI: via [`Session::tick`])
    /// so subsequent reads see freshly-arrived results.
    pub fn drain_inbound(&mut self) {
        while let Ok(event) = self.worker_rx.try_recv() {
            match event {
                WorkerEvent::Print(line) => self.add_status(line),
                WorkerEvent::ExecutionFinished(Ok(stats)) => {
                    self.add_status(format!(
                        "Compute finished: {} nodes, {:.0}s",
                        stats.executed_nodes.len(),
                        stats.elapsed_secs
                    ));
                    self.render_events
                        .push(CacheEvent::InvalidateNodes(invalidated_nodes(&stats)).into());
                    self.execution_stats = Some(stats);
                }
                WorkerEvent::ExecutionFinished(Err(err)) => {
                    self.add_status(format!("Compute failed: {err}"));
                }
                WorkerEvent::ArgumentValues {
                    node_id,
                    values: Some(values),
                } => {
                    self.render_events
                        .push(CacheEvent::Insert(node_id, NodeCache::from(values)).into());
                }
                WorkerEvent::ArgumentValues {
                    node_id,
                    values: None,
                } => {
                    self.render_events
                        .push(CacheEvent::ClearPending(node_id).into());
                }
                WorkerEvent::SavePicked(Some(path)) => self.save_graph(&path),
                WorkerEvent::SavePicked(None) => {}
                WorkerEvent::LoadPicked(Some(path)) => self.load_graph(&path),
                WorkerEvent::LoadPicked(None) => {}
            }
        }

        while let Ok(inbound) = self.script_inbound_rx.try_recv() {
            match inbound {
                SessionInbound::Print { msg } => {
                    self.add_status(format!("script: {msg}"));
                }
                SessionInbound::Apply(intents) => {
                    self.graph_dirty |= self.commit_intents(intents);
                }
                // Run-state changes parked here; `handle_output`
                // consumes them after rendering, so its existing
                // dirty→Update→ExecuteTerminals sequencing applies.
                // Last-script-wins for the same frame; GUI input still
                // takes precedence (see `handle_output`).
                SessionInbound::Run(cmd) => {
                    self.pending_run_cmd = Some(cmd);
                }
                SessionInbound::Shutdown => {
                    self.close_app();
                }
            }
        }
    }

    fn undo(&mut self) {
        let mut affects_computation = false;
        let undid = self.action_stack.undo(&mut self.view_graph, &mut |action| {
            affects_computation |= intent::affects_computation(action);
        });
        self.view_graph.validate();
        if undid && affects_computation {
            self.refresh_graph();
        }
    }

    fn redo(&mut self) {
        let mut affects_computation = false;
        let redid = self.action_stack.redo(&mut self.view_graph, &mut |action| {
            affects_computation |= intent::affects_computation(action);
        });
        self.view_graph.validate();
        if redid && affects_computation {
            self.refresh_graph();
        }
    }

    pub fn handle_output(&mut self, output: &mut FrameOutput) {
        while let Some(err) = output.pop_error() {
            self.add_status(format!("Error: {err}"));
        }

        self.graph_dirty |= self.commit_actions(output);

        // Process undo/redo *after* commit so any actions queued this
        // frame land in the stack first — undo's target is the
        // post-commit state, not the pre-frame state.
        if let Some(cmd) = output.editor_cmd() {
            match cmd {
                EditorCommand::Undo => self.undo(),
                EditorCommand::Redo => self.redo(),
            }
        }

        let mut update_if_dirty = self.autorun();
        let mut msgs: Vec<WorkerMessage> = Vec::new();

        // GUI direct input wins over a script-queued command on
        // conflict — clicking a run button in the same frame as
        // `script: run()` arrives should land the click. Script's
        // pending command is consumed either way.
        let queued_run_cmd = self.pending_run_cmd.take();
        let run_cmd = output.run_cmd().or(queued_run_cmd);
        if let Some(run_cmd) = run_cmd {
            match run_cmd {
                RunCommand::StartAutorun => {
                    msgs.push(WorkerMessage::StartEventLoop);
                    update_if_dirty = true;
                }
                RunCommand::StopAutorun => {
                    msgs.push(WorkerMessage::StopEventLoop);
                }
                RunCommand::RunOnce => {
                    msgs.push(WorkerMessage::ExecuteTerminals);
                    update_if_dirty = true;
                }
            }
        }

        if self.graph_dirty && update_if_dirty {
            msgs.push(WorkerMessage::Update {
                graph: self.view_graph.graph.clone(),
                func_lib: self.func_lib.clone(),
            });
            self.graph_dirty = false;
        }

        // The renderer gates duplicate requests via
        // `ArgumentValuesCache::mark_pending` *before* setting this
        // field — by the time we see it here, the request is already
        // deduped. Session just relays it to the worker.
        if let Some(node_id) = output.request_argument_values() {
            let (reply, rx) = oneshot::channel();
            msgs.push(WorkerMessage::RequestArgumentValues { node_id, reply });
            let ui = self.ui_host.clone();
            let tx = self.worker_tx.clone();
            tokio::spawn(async move {
                if let Ok(values) = rx.await {
                    let _ = tx.send(WorkerEvent::ArgumentValues { node_id, values });
                    ui.request_redraw();
                }
            });
        }

        if !msgs.is_empty()
            && let Some(worker) = &self.worker
        {
            // todo handle
            let _ = worker.send_many(msgs);
        }
    }

    /// Signal the windowing layer to close. Frontend cleanup
    /// (config save, worker exit) still runs through [`Session::exit`]
    /// when the host actually shuts down.
    pub fn close_app(&self) {
        self.ui_host.close_app();
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Replace the in-memory config (e.g. from the Settings window)
    /// and persist it to disk immediately. Transports already running
    /// aren't reconfigured — TCP listener changes apply on next launch.
    pub fn update_config(&mut self, cfg: Config) {
        // Preserve the live `current_path` — it tracks New/Save/Open
        // independently of the settings window, which doesn't edit it.
        let current_path = self.config.current_path.clone();
        self.config = cfg;
        self.config.current_path = current_path;
        self.config.save();
    }

    pub fn exit(&mut self) {
        self.config.save();
        if let Some(worker) = &mut self.worker {
            worker.exit();
        }
        // Drop the executor: cancels its CancellationToken and aborts
        // the executor + transport tasks so sockets/discovery files
        // are released before `run_native` returns.
        self.script_executor = None;
    }

    fn replace_graph(&mut self, view_graph: ViewGraph) {
        self.view_graph = view_graph;
        self.action_stack.clear();
        if let Some(worker) = &self.worker {
            let _ = worker.send(WorkerMessage::Clear);
        }
        // Reset must precede any subsequent cache events queued in the
        // same frame so the renderer's `GraphUi::default()` lands first
        // and later cache events apply to the fresh state.
        self.render_events.push(RenderEvent::Reset);
        self.refresh_graph();
    }

    fn refresh_graph(&mut self) {
        self.view_graph.validate_with(&self.func_lib);
        self.graph_dirty = true;
        self.execution_stats = None;
        self.render_events.push(CacheEvent::Clear.into());
    }

    fn commit_actions(&mut self, output: &mut FrameOutput) -> bool {
        self.commit_intents(output.take_intents())
    }

    /// Apply + record a batch of intents, regardless of source (GUI
    /// frame output, script side-effect, future RPC). The renderer
    /// never mutates `ViewGraph` directly (`GraphContext` holds an
    /// `&ViewGraph`), so this is the one site that writes each intent
    /// exactly once.
    ///
    /// For each intent we capture-then-apply, *moving* the intent into
    /// the resulting `UndoStep` — `AddNode` carries a full `Node` and
    /// avoiding the clone matters for batched spawns. The whole batch
    /// lands as one undo entry. Cross-frame coalescing for continuous
    /// gestures (viewport) happens inside the undo stack via
    /// `intent::gesture_key` — this routine has no pending state,
    /// which is what makes it safe under egui's multi-pass rendering
    /// and equally safe when called from `drain_inbound`.
    fn commit_intents(&mut self, intents: Vec<Intent>) -> bool {
        if intents.is_empty() {
            return false;
        }
        let mut graph_updated = false;
        let mut steps = Vec::with_capacity(intents.len());
        for intent in intents {
            if intent.is_noop_against(&self.view_graph) {
                continue;
            }
            let step = intent::build_step(intent, &self.view_graph);
            intent::apply_step(&step, &mut self.view_graph);
            graph_updated |= intent::affects_computation(&step);
            steps.push(step);
        }
        self.action_stack.clear_redo();
        self.action_stack.push_current(&steps);
        if graph_updated {
            self.refresh_graph();
        }
        graph_updated
    }
}

fn sample_test_hooks(tx: UnboundedSender<WorkerEvent>) -> TestFuncHooks {
    TestFuncHooks {
        get_a: Arc::new(|| Ok(21)),
        get_b: Arc::new(|| 2),
        print: Arc::new(move |value| {
            let _ = tx.send(WorkerEvent::Print(value.to_string()));
        }),
    }
}
