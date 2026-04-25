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
use crate::gui::frame_output::{EditorCommand, FrameOutput, RunCommand};
use crate::gui::graph_ctx::GraphContext;
use crate::model::{ActionStack, ArgumentValuesCache, ViewGraph, graph_ui_action::GraphUiAction};
use crate::script::{self, ScriptAction, ScriptConfig, ScriptExecutor};
use crate::ui_host::UiHost;

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
    argument_values_cache: ArgumentValuesCache,
    status: String,
    config: Config,

    graph_dirty: bool,
    action_stack: ActionStack,

    worker: Option<Worker>,
    worker_tx: UnboundedSender<WorkerEvent>,
    worker_rx: UnboundedReceiver<WorkerEvent>,

    script_executor: Option<ScriptExecutor>,
    script_action_rx: UnboundedReceiver<ScriptAction>,

    ui_host: Arc<dyn UiHost>,
}

impl Session {
    pub fn new<H: UiHost + 'static>(ui_host: H, script_config: ScriptConfig) -> Self {
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

        let (script_action_tx, script_action_rx) = unbounded_channel::<ScriptAction>();
        let transports = script::build_transports(&script_config);
        let script_executor = ScriptExecutor::new(transports, script_action_tx);

        let mut result = Self::from_parts(
            func_lib,
            Config::load_or_default(),
            Some(worker),
            worker_tx,
            worker_rx,
            Some(script_executor),
            script_action_rx,
            ui_host,
        );

        if let Some(path) = result.config.current_path.clone() {
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
        script_action_rx: UnboundedReceiver<ScriptAction>,
        ui_host: Arc<dyn UiHost>,
    ) -> Self {
        Self {
            func_lib,
            view_graph: ViewGraph::default(),
            execution_stats: None,
            argument_values_cache: ArgumentValuesCache::default(),
            status: String::new(),
            config,
            graph_dirty: true,
            action_stack: ActionStack::new(UNDO_MAX_STEPS),
            worker,
            worker_tx,
            worker_rx,
            script_action_rx,
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

    /// Frame-level dependency bundle for the view layer. `view_graph`
    /// is shared-borrowed (mutations go through `GraphUiAction::apply`
    /// in `commit_actions`); `argument_values_cache` is `&mut` because
    /// rendering lazily fills it with texture handles.
    pub fn graph_context(&mut self) -> GraphContext<'_> {
        let execution_stats = self.execution_stats.as_ref();
        GraphContext {
            func_lib: &self.func_lib,
            view_graph: &self.view_graph,
            execution_stats,
            exec_info_index: crate::model::NodeExecutionIndex::new(execution_stats),
            autorun: self.autorun(),
            argument_values_cache: &mut self.argument_values_cache,
        }
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

    /// Applies the emitted actions to `view_graph` in order.
    /// Returns `true` if any applied action affects computation.
    ///
    /// Does *not* record undo history — that's the job of
    /// [`Session::commit_actions`].
    pub fn apply(&mut self, actions: &[GraphUiAction]) -> bool {
        let mut graph_updated = false;
        for action in actions {
            action.apply(&mut self.view_graph);
            graph_updated |= action.affects_computation();
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
    /// Frontends must call this once per frame before rendering.
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
                    self.argument_values_cache.invalidate_changed(&stats);
                    self.execution_stats = Some(stats);
                }
                WorkerEvent::ExecutionFinished(Err(err)) => {
                    self.add_status(format!("Compute failed: {err}"));
                }
                WorkerEvent::ArgumentValues {
                    node_id,
                    values: Some(values),
                } => {
                    self.argument_values_cache.insert(node_id, values.into());
                }
                WorkerEvent::ArgumentValues {
                    node_id,
                    values: None,
                } => {
                    self.argument_values_cache.clear_pending(node_id);
                }
                WorkerEvent::SavePicked(Some(path)) => self.save_graph(&path),
                WorkerEvent::SavePicked(None) => {}
                WorkerEvent::LoadPicked(Some(path)) => self.load_graph(&path),
                WorkerEvent::LoadPicked(None) => {}
            }
        }

        while let Ok(action) = self.script_action_rx.try_recv() {
            match action {
                ScriptAction::Print { origin, msg } => {
                    self.add_status(format!("remote {origin}: {msg}"));
                }
            }
        }
    }

    fn undo(&mut self) {
        let mut affects_computation = false;
        let undid = self.action_stack.undo(&mut self.view_graph, &mut |action| {
            affects_computation |= action.affects_computation();
        });
        self.view_graph.validate();
        if undid && affects_computation {
            self.refresh_graph();
        }
    }

    fn redo(&mut self) {
        let mut affects_computation = false;
        let redid = self.action_stack.redo(&mut self.view_graph, &mut |action| {
            affects_computation |= action.affects_computation();
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

        if let Some(run_cmd) = output.run_cmd() {
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

        if let Some(node_id) = output.request_argument_values()
            && self.argument_values_cache.mark_pending(node_id)
        {
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
        self.refresh_graph();
    }

    fn refresh_graph(&mut self) {
        self.view_graph.validate_with(&self.func_lib);
        self.graph_dirty = true;
        self.execution_stats = None;
        self.argument_values_cache.clear();
    }

    fn commit_actions(&mut self, output: &mut FrameOutput) -> bool {
        let actions = output.actions();
        if actions.is_empty() {
            return false;
        }

        // Apply + record. Renderer never mutates ViewGraph (GraphContext
        // holds &ViewGraph), so this is the one site that writes each
        // action exactly once. Cross-frame coalescing for continuous
        // gestures (zoom/pan) happens at the undo-stack level via
        // `GraphUiAction::gesture_key` — this routine has no pending
        // state, which is what makes it safe under egui's multi-pass
        // rendering.
        let graph_updated = self.apply(actions);
        self.action_stack.clear_redo();
        self.action_stack.push_current(actions);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ViewNode;
    use crate::ui_host::UiHost;
    use egui::Pos2;

    #[derive(Debug)]
    struct NoopUiHost;
    impl UiHost for NoopUiHost {
        fn request_redraw(&self) {}
        fn close_app(&self) {}
    }

    /// Stub-backed Session for unit tests: no tokio runtime, no
    /// network listener, no config autoload.
    fn test_session() -> Session {
        let (worker_tx, worker_rx) = unbounded_channel::<WorkerEvent>();
        let (_script_tx, script_action_rx) = unbounded_channel::<ScriptAction>();
        Session::from_parts(
            Arc::new(FuncLib::default()),
            Config::default(),
            None,
            worker_tx,
            worker_rx,
            None,
            script_action_rx,
            Arc::new(NoopUiHost),
        )
    }

    #[test]
    fn add_status_first_line_has_no_leading_newline() {
        let mut session = test_session();
        session.add_status("hello");
        assert_eq!(session.status(), "hello");
    }

    #[test]
    fn add_status_appends_with_newline_separator() {
        let mut session = test_session();
        session.add_status("one");
        session.add_status("two");
        assert_eq!(session.status(), "one\ntwo");
    }

    #[test]
    fn add_status_caps_buffer_to_2000_chars() {
        let mut session = test_session();
        for _ in 0..300 {
            session.add_status("0123456789");
        }
        assert!(session.status().len() <= STATUS_CAP);
    }

    #[test]
    fn apply_reports_when_action_affects_computation() {
        let mut session = test_session();

        // NodeSelected is a UI-only action — should NOT affect computation.
        let affects = session.apply(&[GraphUiAction::NodeSelected {
            before: None,
            after: None,
        }]);
        assert!(!affects);

        // NodeMoved is also UI-only.
        let node_id = NodeId::unique();
        session.view_graph.view_nodes.add(ViewNode {
            id: node_id,
            pos: Pos2::ZERO,
        });
        let affects = session.apply(&[GraphUiAction::NodeMoved {
            node_id,
            before: Pos2::ZERO,
            after: Pos2::new(1.0, 2.0),
        }]);
        assert!(!affects);
        assert_eq!(
            session.view_graph.view_nodes.by_key(&node_id).unwrap().pos,
            Pos2::new(1.0, 2.0)
        );
    }
}
