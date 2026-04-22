use crate::common::undo_stack::UndoStack;
use crate::editor_funclib::EditorFuncLib;
use crate::gui::frame_output::{FrameOutput, RunCommand};
use crate::model::ActionUndoStack;
use crate::model::ArgumentValuesCache;
use crate::model::config::Config;
use crate::model::graph_ui_action::GraphUiAction;
use anyhow::Result;
use common::slot::Slot;
use common::{SerdeFormat, Shared};
use palantir::ImageFuncLib;
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::{FRAME_EVENT_FUNC_ID, WorkerEventsFuncLib};
use scenarium::execution_graph::{self, Result as ExecutionGraphResult};
use scenarium::graph::{Binding, Node, NodeId};
use scenarium::prelude::{ExecutionStats, FuncId, FuncLib};
use scenarium::prelude::{TestFuncHooks, test_func_lib, test_graph};
use scenarium::worker::{ArgumentValuesCallback, WorkerMessage};
use scenarium::worker::{EventRef, ProcessingCallback, Worker};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::{Notify, watch};

use crate::main_ui::UiContext;
use crate::model::{ViewGraph, ViewNode};

use scenarium::execution_graph::ArgumentValues;

const UNDO_MAX_STEPS: usize = 256;

/// Pure, testable domain state. No worker, no tokio, no async channels —
/// anything in here can be constructed in a unit test, poked with actions,
/// and inspected.
///
/// `AppData` wraps this with the session-level facilities (worker channels,
/// undo stack, graph-dirty flag, redraw hook).
#[derive(Debug, Default)]
pub struct AppState {
    pub func_lib: FuncLib,
    pub view_graph: ViewGraph,
    pub execution_stats: Option<ExecutionStats>,
    pub argument_values_cache: ArgumentValuesCache,
    pub status: String,
    pub config: Config,
    pub autorun: bool,
}

impl AppState {
    /// Appends a status line. Keeps the buffer below a 2000-char cap by
    /// draining oldest content.
    pub fn add_status(&mut self, message: impl AsRef<str>) {
        if !self.status.is_empty() {
            self.status.push('\n');
        }
        self.status.push_str(message.as_ref());
        if self.status.len() > 2000 {
            self.status.drain(..self.status.len() - 2000);
        }
    }

    /// Applies the emitted actions to `view_graph` in order (idempotent
    /// apply contract; see `GraphUiAction::apply`). Returns `true` if any
    /// applied action affects computation.
    ///
    /// Does *not* record undo history — that's a session concern, handled
    /// by [`AppData::handle_actions`].
    pub fn apply_actions(&mut self, actions: &[GraphUiAction]) -> bool {
        let mut graph_updated = false;
        for action in actions {
            action.apply(&mut self.view_graph);
            graph_updated |= action.affects_computation();
        }
        graph_updated
    }

    /// Resets execution caches after a mutation. Caller is responsible for
    /// telling the worker to re-run (that's `AppData::graph_dirty`).
    fn clear_execution_caches(&mut self) {
        self.execution_stats = None;
        self.argument_values_cache.clear();
    }
}

#[derive(Debug)]
pub struct AppData {
    pub state: AppState,

    worker: Worker,

    pub ui_context: UiContext,

    graph_dirty: bool,

    undo_stack: Box<dyn UndoStack<ViewGraph, Action = GraphUiAction>>,

    execution_stats_rx: Slot<Result<ExecutionStats, execution_graph::Error>>,
    argument_values_rx: Slot<(NodeId, Option<ArgumentValues>)>,
    print_out_rx: UnboundedReceiver<String>,
}

impl AppData {
    pub fn new(ui_context: UiContext) -> Self {
        let config = Config::load_or_default();

        let (worker, execution_stats_rx) = Self::create_worker(ui_context.clone());
        let argument_values_rx = Slot::default();
        let (print_out_tx, print_out_rx) = unbounded_channel::<String>();

        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(sample_test_hooks(print_out_tx)));
        func_lib.merge(EditorFuncLib::default());
        func_lib.merge(BasicFuncLib::default());
        func_lib.merge(WorkerEventsFuncLib::default());
        func_lib.merge(ImageFuncLib::default());

        let state = AppState {
            func_lib,
            view_graph: ViewGraph::default(),
            execution_stats: None,
            argument_values_cache: ArgumentValuesCache::default(),
            status: String::new(),
            config,
            autorun: false,
        };

        let mut result = Self {
            state,
            worker,
            graph_dirty: true,
            undo_stack: Box::new(ActionUndoStack::new(UNDO_MAX_STEPS)),
            ui_context,
            execution_stats_rx,
            argument_values_rx,
            print_out_rx,
        };

        if let Some(path) = result.state.config.current_path.clone() {
            result.load_graph(&path);
        }

        result
    }

    pub fn empty_graph(&mut self) {
        self.replace_graph(ViewGraph::default(), true);
        self.state.add_status("Created new graph");
    }

    pub fn save_graph(&mut self, path: &Path) {
        fn save_to_file(state: &AppState, path: &Path) -> Result<()> {
            let format = SerdeFormat::from_file_name(path.to_string_lossy().as_ref())
                .map_err(anyhow::Error::from)?;
            let payload = state.view_graph.serialize(format);
            std::fs::write(path, payload).map_err(anyhow::Error::from)
        }

        match save_to_file(&self.state, path) {
            Ok(()) => {
                self.state.config.current_path = Some(path.to_path_buf());
                self.state
                    .add_status(format!("Saved graph to {}", path.display()));
            }
            Err(err) => self
                .state
                .add_status(format!("Save failed: {} {err}", path.display())),
        }
    }

    pub fn load_graph(&mut self, path: &Path) {
        fn load_from_file(this: &mut AppData, path: &Path) -> Result<()> {
            let format = SerdeFormat::from_file_name(path.to_string_lossy().as_ref())
                .map_err(anyhow::Error::from)?;
            let payload = std::fs::read(path).map_err(anyhow::Error::from)?;
            this.replace_graph(ViewGraph::deserialize(format, &payload)?, true);

            Ok(())
        }

        match load_from_file(self, path) {
            Ok(()) => {
                self.state.config.current_path = Some(path.to_path_buf());
                self.state
                    .add_status(format!("Loaded graph from {}", path.display()));
            }
            Err(err) => {
                self.state.config.current_path = None;
                self.state
                    .add_status(format!("Load failed: {} {err}", path.display()));
            }
        }
    }

    pub fn update_shared_status(&mut self) {
        self.state.autorun = self.worker.is_event_loop_started();

        loop {
            let result = self.print_out_rx.try_recv();
            match result {
                Ok(print_out) => self.state.add_status(print_out),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => panic!("Print output channel disconnected"),
            }
        }

        if let Some(execution_stats) = self.execution_stats_rx.take() {
            match execution_stats {
                Ok(execution_stats) => {
                    let message = format!(
                        "Compute finished: {} nodes, {:.0}s",
                        execution_stats.executed_nodes.len(),
                        execution_stats.elapsed_secs
                    );

                    self.state
                        .argument_values_cache
                        .invalidate_changed(&execution_stats);
                    self.state.execution_stats = Some(execution_stats);

                    self.state.status.push('\n');
                    self.state.status.push_str(&message);
                }
                Err(err) => {
                    self.state.status.push('\n');
                    self.state
                        .status
                        .push_str(&format!("Compute failed: {err}"));
                }
            }
        }

        // Process argument values response
        if let Some((node_id, values)) = self.argument_values_rx.take() {
            if let Some(values) = values {
                self.state
                    .argument_values_cache
                    .insert(node_id, values.into());
            } else {
                self.state.argument_values_cache.clear_pending(node_id);
            }
        }
    }

    pub fn undo(&mut self, output: &mut FrameOutput) {
        // Commit anything queued this frame before stepping back so
        // the undo target is the pre-current-frame state.
        self.handle_actions(output);

        let mut affects_computation = false;
        let undid = self
            .undo_stack
            .undo(&mut self.state.view_graph, &mut |action| {
                affects_computation |= action.affects_computation();
            });
        self.state.view_graph.validate();
        if undid && affects_computation {
            self.refresh_graph();
        }
    }

    pub fn redo(&mut self) {
        let mut affects_computation = false;
        let redid = self
            .undo_stack
            .redo(&mut self.state.view_graph, &mut |action| {
                affects_computation |= action.affects_computation();
            });
        self.state.view_graph.validate();
        if redid && affects_computation {
            self.refresh_graph();
        }
    }

    pub fn handle_output(&mut self, output: &mut FrameOutput) {
        while let Some(err) = output.pop_error() {
            self.state.add_status(format!("Error: {err}"));
        }

        self.graph_dirty |= self.handle_actions(output);

        let mut update_if_dirty = self.state.autorun;
        let mut msgs: Vec<WorkerMessage> = Vec::default();

        if let Some(run_cmd) = output.run_cmd() {
            match run_cmd {
                RunCommand::StartAutorun => {
                    assert!(!self.state.autorun);

                    msgs.push(WorkerMessage::StartEventLoop);
                    update_if_dirty = true;
                    self.state.autorun = true;
                }
                RunCommand::StopAutorun => {
                    assert!(self.state.autorun);

                    msgs.push(WorkerMessage::StopEventLoop);
                    self.state.autorun = false;
                }
                RunCommand::RunOnce => {
                    msgs.push(WorkerMessage::ExecuteTerminals);
                    update_if_dirty = true;
                }
            }
        }

        if self.graph_dirty && update_if_dirty {
            msgs.push(WorkerMessage::Update {
                graph: self.state.view_graph.graph.clone(),
                func_lib: self.state.func_lib.clone(),
            });
            self.graph_dirty = false;
        }

        // Handle argument values request (only if not already pending)
        if let Some(node_id) = output.request_argument_values()
            && self.state.argument_values_cache.mark_pending(node_id)
        {
            msgs.push(WorkerMessage::RequestArgumentValues {
                node_id,
                callback: ArgumentValuesCallback::new({
                    let ui_context = self.ui_context.clone();
                    let slot = self.argument_values_rx.clone();

                    move |values| {
                        slot.send((node_id, values));
                        ui_context.request_redraw();
                    }
                }),
            });
        }

        if !msgs.is_empty() {
            self.worker.send_many(msgs);
        }
    }

    pub fn exit(&mut self) {
        self.state.config.save();
        self.worker.exit();
    }

    fn create_worker(
        ui_refresh: UiContext,
    ) -> (Worker, Slot<Result<ExecutionStats, execution_graph::Error>>) {
        let slot = Slot::default();

        (
            Worker::new({
                let slot = slot.clone();
                move |result| {
                    slot.send(result);
                    ui_refresh.request_redraw();
                }
            }),
            slot,
        )
    }

    fn replace_graph(&mut self, view_graph: ViewGraph, reset_undo: bool) {
        // todo!();
        // view_graph.update_from_func_lib(&self.func_lib);

        self.state.view_graph = view_graph;

        if reset_undo {
            self.undo_stack.reset_with(&self.state.view_graph);
        }

        self.worker.send(WorkerMessage::Clear);
        self.refresh_graph();
    }

    fn refresh_graph(&mut self) {
        self.state.view_graph.validate_with(&self.state.func_lib);

        self.graph_dirty = true;
        self.state.clear_execution_caches();
    }

    fn handle_actions(&mut self, output: &mut FrameOutput) -> bool {
        let mut graph_updated = false;

        for actions in output.action_stacks() {
            // Apply + record. Idempotent apply means mutations that
            // already happened inline during render are no-ops; the
            // single source of truth for mutations that intentionally
            // defer lives here. Cross-frame coalescing for continuous
            // gestures (zoom/pan) happens at the undo stack level via
            // `GraphUiAction::gesture_key`, not here, so this loop
            // doesn't need any pending-action state — which is what
            // makes it safe under egui's multi-pass rendering.
            let any_affecting = self.state.apply_actions(actions);
            self.undo_stack.clear_redo();
            self.undo_stack
                .push_current(&self.state.view_graph, actions);

            graph_updated |= any_affecting;
        }

        if graph_updated {
            self.refresh_graph();
        }

        graph_updated
    }
}

fn sample_test_hooks(print_out_tx: UnboundedSender<String>) -> TestFuncHooks {
    TestFuncHooks {
        get_a: Arc::new(|| Ok(21)),
        get_b: Arc::new(|| 2),
        print: Arc::new(move |value| {
            print_out_tx.send(value.to_string()).unwrap();
        }),
    }
}

fn add_node_from_func_id(view_graph: &mut ViewGraph, func_lib: &FuncLib, func_id: FuncId) {
    if view_graph
        .graph
        .nodes
        .iter_mut()
        .all(|node| node.func_id != func_id)
    {
        let func = func_lib.by_id(&func_id).unwrap();
        view_graph.add_node_from_func(func);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ViewNode;
    use egui::Pos2;

    // Pure AppState tests — no worker, no tokio, no egui.

    #[test]
    fn add_status_first_line_has_no_leading_newline() {
        let mut state = AppState::default();
        state.add_status("hello");
        assert_eq!(state.status, "hello");
    }

    #[test]
    fn add_status_appends_with_newline_separator() {
        let mut state = AppState::default();
        state.add_status("one");
        state.add_status("two");
        assert_eq!(state.status, "one\ntwo");
    }

    #[test]
    fn add_status_caps_buffer_to_2000_chars() {
        let mut state = AppState::default();
        // Push ~3000 chars; each add is appended with a newline.
        for _ in 0..300 {
            state.add_status("0123456789");
        }
        assert!(state.status.len() <= 2000);
    }

    #[test]
    fn apply_actions_reports_when_action_affects_computation() {
        let mut state = AppState::default();

        // NodeSelected is a UI-only action — should NOT report as affecting computation.
        let selected_action = GraphUiAction::NodeSelected {
            before: None,
            after: None,
        };
        let affects = state.apply_actions(&[selected_action]);
        assert!(!affects);

        // NodeMoved is also UI-only.
        let node_id = NodeId::unique();
        let mut vg = ViewGraph::default();
        vg.view_nodes.add(ViewNode {
            id: node_id,
            pos: Pos2::ZERO,
        });
        state.view_graph = vg;
        let moved = GraphUiAction::NodeMoved {
            node_id,
            before: Pos2::ZERO,
            after: Pos2::new(1.0, 2.0),
        };
        let affects = state.apply_actions(&[moved]);
        assert!(!affects);
        assert_eq!(
            state.view_graph.view_nodes.by_key(&node_id).unwrap().pos,
            Pos2::new(1.0, 2.0)
        );
    }
}
