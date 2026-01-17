use crate::common::undo_stack::{ActionUndoStack, UndoStack};
use crate::elements::editor_funclib::EditorFuncLib;
use crate::gui::graph_ui_interaction::{GraphUiInteraction, RunCommand};
use crate::model::ArgumentValuesCache;
use crate::model::config::Config;
use crate::model::graph_ui_action::GraphUiAction;
use anyhow::Result;
use common::lambda::Lambda;
use common::slot::Slot;
use common::{SerdeFormat, Shared};
use graph::elements::basic_funclib::BasicFuncLib;
use graph::elements::timers_funclib::{FRAME_EVENT_FUNC_ID, TimersFuncLib};
use graph::execution_graph::{self, Result as ExecutionGraphResult};
use graph::graph::{Binding, Node, NodeId};
use graph::prelude::{ExecutionStats, FuncId, FuncLib};
use graph::prelude::{TestFuncHooks, test_func_lib, test_graph};
use graph::worker::{ArgumentValuesCallback, WorkerMessage};
use graph::worker::{EventRef, ProcessingCallback, Worker};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::{Notify, watch};

use crate::main_ui::UiContext;
use crate::model::{ViewGraph, ViewNode};

use graph::execution_graph::ArgumentValues;

const UNDO_MAX_STEPS: usize = 256;

#[derive(Debug)]
pub struct AppData {
    pub func_lib: FuncLib,
    pub view_graph: ViewGraph,
    pub interaction: GraphUiInteraction,
    pub execution_stats: Option<ExecutionStats>,
    pub argument_values_cache: ArgumentValuesCache,

    pub status: String,

    pub config: Config,

    worker: Worker,

    pub ui_context: UiContext,

    pub autorun: bool,
    graph_dirty: bool,

    undo_stack: Box<dyn UndoStack<ViewGraph, Action = GraphUiAction>>,

    pub run_event: Arc<Notify>,
    pub reset_frame_event: Lambda,

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

        let timers_func_lib = TimersFuncLib::default();
        let reset_frame_no = timers_func_lib.reset_frame_event.clone();
        let run_event = timers_func_lib.run_event.clone();

        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(sample_test_hooks(print_out_tx)));
        func_lib.merge(EditorFuncLib::default());
        func_lib.merge(BasicFuncLib::default());
        func_lib.merge(timers_func_lib);

        let mut result = Self {
            func_lib,
            view_graph: ViewGraph::default(),
            interaction: GraphUiInteraction::default(),
            execution_stats: None,
            argument_values_cache: ArgumentValuesCache::default(),
            config,
            worker,

            autorun: false,
            graph_dirty: true,

            undo_stack: Box::new(ActionUndoStack::new(UNDO_MAX_STEPS)),

            status: String::new(),

            ui_context,
            execution_stats_rx,
            argument_values_rx,
            print_out_rx,

            run_event,
            reset_frame_event: reset_frame_no,
        };

        if let Some(path) = result.config.current_path.clone() {
            result.load_graph(&path);
        }

        result
    }

    pub fn empty_graph(&mut self) {
        self.apply_graph(ViewGraph::default(), true);
        self.add_status("Created new graph");
    }

    pub fn save_graph(&mut self, path: &Path) {
        fn save_to_file(this: &mut AppData, path: &Path) -> Result<()> {
            let format = SerdeFormat::from_file_name(path.to_string_lossy().as_ref())
                .map_err(anyhow::Error::from)?;
            let payload = this.view_graph.serialize(format);
            std::fs::write(path, payload).map_err(anyhow::Error::from)
        }

        match save_to_file(self, path) {
            Ok(()) => {
                self.config.current_path = Some(path.to_path_buf());
                self.add_status(format!("Saved graph to {}", path.display()));
            }
            Err(err) => self.add_status(format!("Save failed: {} {err}", path.display())),
        }
    }

    pub fn load_graph(&mut self, path: &Path) {
        fn load_from_file(this: &mut AppData, path: &Path) -> Result<()> {
            let format = SerdeFormat::from_file_name(path.to_string_lossy().as_ref())
                .map_err(anyhow::Error::from)?;
            let payload = std::fs::read(path).map_err(anyhow::Error::from)?;
            this.apply_graph(ViewGraph::deserialize(format, &payload)?, true);

            Ok(())
        }

        match load_from_file(self, path) {
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

    pub fn load_test_graph(&mut self) {
        let graph = test_graph();
        let mut view_graph: ViewGraph = graph.into();

        add_node_from_func_id(&mut view_graph, &self.func_lib, TimersFuncLib::RUN_FUNC_ID);
        add_node_from_func_id(&mut view_graph, &self.func_lib, FRAME_EVENT_FUNC_ID);

        view_graph.auto_place_nodes();
        self.apply_graph(view_graph, true);

        self.add_status("Loaded sample test graph");
    }

    pub fn update_shared_status(&mut self) {
        loop {
            let result = self.print_out_rx.try_recv();
            match result {
                Ok(print_out) => self.add_status(print_out),
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

                    self.argument_values_cache
                        .invalidate_changed(&execution_stats);
                    self.execution_stats = Some(execution_stats);

                    self.status.push('\n');
                    self.status.push_str(&message);
                }
                Err(err) => {
                    self.status.push('\n');
                    self.status.push_str(&format!("Compute failed: {err}"));
                }
            }
        }

        // Process argument values response
        if let Some((node_id, Some(values))) = self.argument_values_rx.take() {
            self.argument_values_cache.insert(node_id, values);
        }
    }

    pub fn undo(&mut self) {
        self.interaction.flush();
        self.handle_actions();

        let mut affects_computation = false;
        let undid = self.undo_stack.undo(&mut self.view_graph, &mut |action| {
            affects_computation |= action.affects_computation();
        });
        self.view_graph.validate();
        if undid && affects_computation {
            self.reinit_graph();
        }
    }

    pub fn redo(&mut self) {
        let mut affects_computation = false;
        let redid = self.undo_stack.redo(&mut self.view_graph, &mut |action| {
            affects_computation |= action.affects_computation();
        });
        self.view_graph.validate();
        if redid && affects_computation {
            self.reinit_graph();
        }
    }

    pub fn handle_interaction(&mut self) {
        while let Some(err) = self.interaction.errors.pop() {
            self.add_status(format!("Error: {err}"));
        }

        self.graph_dirty |= self.handle_actions();

        let mut msgs: Vec<WorkerMessage> = Vec::default();
        let mut run_once: bool = false;

        match self.interaction.run_cmd {
            RunCommand::StartAutorun => {
                if !self.autorun {
                    self.reset_frame_event.call();
                    msgs.push(WorkerMessage::StartEventLoop);
                }
                self.autorun = true;
            }
            RunCommand::StopAutorun => {
                msgs.push(WorkerMessage::StopEventLoop);
                self.autorun = false;
            }
            RunCommand::None => {}
            RunCommand::RunOnce => {
                self.reset_frame_event.call();
                run_once = true;
            }
        }

        let update_graph = self.graph_dirty && (self.autorun || run_once);

        if update_graph {
            msgs.push(WorkerMessage::Update {
                graph: self.view_graph.graph.clone(),
                func_lib: self.func_lib.clone(),
            });
            self.graph_dirty = false;
        }

        if run_once || (self.autorun && update_graph) {
            msgs.push(WorkerMessage::ExecuteTerminals);
        }

        // Handle argument values request
        if let Some(node_id) = self.interaction.request_argument_values {
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
            self.worker.send(WorkerMessage::Multi { msgs });
        }

        self.interaction.clear();
    }

    pub fn exit(&mut self) {
        self.config.save();
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

    fn add_status(&mut self, message: impl AsRef<str>) {
        if !self.status.is_empty() {
            self.status.push('\n');
        }
        self.status.push_str(message.as_ref());
        if self.status.len() > 2000 {
            self.status.drain(..self.status.len() - 2000);
        }
    }

    fn apply_graph(&mut self, view_graph: ViewGraph, reset_undo: bool) {
        // todo!();
        // view_graph.update_from_func_lib(&self.func_lib);

        self.view_graph = view_graph;

        if reset_undo {
            self.undo_stack.reset_with(&self.view_graph);
        }

        self.reinit_graph();
    }

    fn reinit_graph(&mut self) {
        self.view_graph.validate_with(&self.func_lib);

        self.graph_dirty = true;
        self.execution_stats = None;
        self.argument_values_cache.clear();
        self.reset_frame_event.call();
        self.worker.send(WorkerMessage::Clear);
    }

    fn handle_actions(&mut self) -> bool {
        let mut graph_updated = false;

        for actions in self.interaction.action_stacks() {
            self.undo_stack.clear_redo();
            self.undo_stack.push_current(&self.view_graph, actions);

            if actions.iter().any(|action| action.affects_computation()) {
                graph_updated = true;
            }
        }

        if graph_updated {
            self.reinit_graph();
        }

        graph_updated
    }
}

fn sample_test_hooks(print_out_tx: UnboundedSender<String>) -> TestFuncHooks {
    TestFuncHooks {
        get_a: Arc::new(|| 21),
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
