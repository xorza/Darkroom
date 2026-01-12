use crate::common::undo_stack::{ActionUndoStack, UndoStack};
use crate::elements::editor_funclib::EditorFuncLib;
use crate::gui::graph_ui_interaction::{GraphUiAction, GraphUiInteraction};
use anyhow::Result;
use common::{FileFormat, Shared};
use graph::elements::timers_funclib::{FRAME_EVENT_FUNC_ID, TimersFuncLib};
use graph::execution_graph::Result as ExecutionGraphResult;
use graph::graph::{Binding, Node, NodeId};
use graph::prelude::{ExecutionStats, FuncId, FuncLib};
use graph::prelude::{TestFuncHooks, test_func_lib, test_graph};
use graph::worker::WorkerMessage;
use graph::worker::{EventId, Worker};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Notify;

use crate::main_ui::UiContext;
use crate::model::{ViewGraph, ViewNode};

#[derive(Debug, Default)]
pub struct Status {
    execution_stats: Option<ExecutionGraphResult<ExecutionStats>>,
    print_output: Option<String>,
}

pub type SharedStatus = Shared<Status>;

const UNDO_MAX_STEPS: usize = 256;

#[derive(Debug)]
pub struct AppData {
    pub worker: Worker,
    pub func_lib: FuncLib,
    pub view_graph: ViewGraph,
    pub interaction: GraphUiInteraction,
    pub execution_stats: Option<ExecutionStats>,
    pub graph_updated: bool,
    pub current_path: PathBuf,
    pub status: String,
    pub _ui_context: UiContext,

    pub shared_status: SharedStatus,
    pub run_event: Arc<Notify>,

    undo_stack: Box<dyn UndoStack<ViewGraph, Action = GraphUiAction>>,
}

impl AppData {
    pub fn new(ui_context: UiContext, current_path: PathBuf) -> Self {
        let shared_status = Shared::default();
        let worker = Self::create_worker(shared_status.clone(), ui_context.clone());

        let run_event = Arc::new(Notify::new());

        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(sample_test_hooks(shared_status.clone())));
        func_lib.merge(TimersFuncLib::default());
        func_lib.merge(EditorFuncLib::new(Arc::clone(&run_event)));

        Self {
            worker,
            func_lib,
            view_graph: ViewGraph::default(),
            interaction: GraphUiInteraction::default(),
            execution_stats: None,
            graph_updated: false,
            current_path,
            status: String::new(),
            _ui_context: ui_context,

            shared_status,
            run_event,

            undo_stack: Box::new(ActionUndoStack::new(UNDO_MAX_STEPS)),
        }
    }

    fn create_worker(shared_status: SharedStatus, ui_refresh: UiContext) -> Worker {
        Worker::new(move |result| {
            let mut shared_status = shared_status.try_lock().unwrap();
            shared_status.execution_stats = Some(result);

            ui_refresh.request_redraw();
        })
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
    }

    pub fn save_graph(&mut self) {
        match self.save_to_file(&self.current_path) {
            Ok(()) => self.set_status(format!("Saved graph to {}", self.current_path.display())),
            Err(err) => self.set_status(format!("Save failed: {err}")),
        }
    }

    pub fn load_graph(&mut self) {
        match self.load_from_file(&self.current_path.clone()) {
            Ok(()) => self.set_status(format!("Loaded graph from {}", self.current_path.display())),
            Err(err) => self.set_status(format!("Load failed: {err}")),
        }
    }

    pub fn run_graph(&mut self) {
        if self.view_graph.graph.nodes.is_empty() {
            self.set_status("Run failed: no graph loaded");
            return;
        }

        if self.graph_updated {
            self.worker
                .update(self.view_graph.graph.clone(), self.func_lib.clone());
            self.worker.send(WorkerMessage::StartEventLoop);
            self.graph_updated = false;
        }

        // let events: Vec<EventId> = self
        //     .view_graph
        //     .graph
        //     .nodes
        //     .iter()
        //     .filter(|node| node.func_id == EditorFuncLib::RUN_FUNC_ID)
        //     .map(|node| EventId {
        //         node_id: node.id,
        //         event_idx: 0,
        //     })
        //     .collect();
        // if events.is_empty() {
        //     self.worker.execute_terminals();
        // } else {
        //     self.worker.execute_events(events);
        // }
        self.run_event.notify_waiters();
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
            self.refresh_after_graph_change();
        }
    }

    pub fn redo(&mut self) {
        let mut affects_computation = false;
        let redid = self.undo_stack.redo(&mut self.view_graph, &mut |action| {
            affects_computation |= action.affects_computation();
        });
        self.view_graph.validate();
        if redid && affects_computation {
            self.refresh_after_graph_change();
        }
    }

    pub fn apply_graph(&mut self, view_graph: ViewGraph, reset_undo: bool) {
        self.view_graph = view_graph;

        if reset_undo {
            self.undo_stack.reset_with(&self.view_graph);
        }
        self.refresh_after_graph_change();
    }

    fn refresh_after_graph_change(&mut self) {
        self.worker.send(WorkerMessage::Clear);
        self.graph_updated = true;
        self.execution_stats = None;
    }

    pub fn handle_interaction(&mut self) {
        self.handle_actions();

        if self.interaction.run {
            self.run_graph();
        }

        if let Some(err) = self.interaction.errors.last() {
            self.set_status(format!("Graph error: {err}"));
        }

        self.interaction.clear();
    }

    fn handle_actions(&mut self) {
        for actions in self.interaction.actions_stacks() {
            self.undo_stack.clear_redo();
            self.undo_stack.push_current(&self.view_graph, actions);

            if actions.iter().any(|action| action.affects_computation()) {
                self.execution_stats = None;
                self.graph_updated = true;
            }
        }
        self.interaction.clear_actions();
    }

    pub fn empty_graph(&mut self) {
        self.apply_graph(ViewGraph::default(), true);
        self.set_status("Created new graph");
    }

    pub fn load_test_graph(&mut self) {
        let graph = test_graph();
        let mut view_graph: ViewGraph = graph.into();

        add_node_from_func_id(&mut view_graph, &self.func_lib, EditorFuncLib::RUN_FUNC_ID);
        add_node_from_func_id(&mut view_graph, &self.func_lib, FRAME_EVENT_FUNC_ID);

        view_graph.auto_place_nodes();
        self.apply_graph(view_graph, true);

        self.set_status("Loaded sample test graph");
    }

    pub fn update_status(&mut self) {
        let mut shared_status = self.shared_status.try_lock().unwrap();
        let print_out = shared_status.print_output.take();

        if let Some(execution_stats) = shared_status.execution_stats.take() {
            match execution_stats {
                Ok(execution_stats) => {
                    self.execution_stats = Some(execution_stats);

                    let summary = format!(
                        "({} nodes, {:.0}s)",
                        self.execution_stats.as_ref().unwrap().executed_nodes.len(),
                        self.execution_stats.as_ref().unwrap().elapsed_secs
                    );

                    let message = if let Some(print_output) = print_out {
                        format!("Compute output: {print_output} {summary}")
                    } else {
                        format!("Compute finished {summary}")
                    };

                    self.status = message;
                }
                Err(err) => {
                    self.status = format!("Compute failed: {err}");
                }
            }
        }
    }

    fn save_to_file(&self, path: &Path) -> Result<()> {
        let format = FileFormat::from_file_name(path.to_string_lossy().as_ref())
            .map_err(anyhow::Error::from)?;
        let payload = self.view_graph.serialize(format);
        std::fs::write(path, payload).map_err(anyhow::Error::from)
    }

    fn load_from_file(&mut self, path: &Path) -> Result<()> {
        let format = FileFormat::from_file_name(path.to_string_lossy().as_ref())
            .map_err(anyhow::Error::from)?;
        let payload = std::fs::read(path).map_err(anyhow::Error::from)?;
        self.apply_graph(ViewGraph::deserialize(format, &payload)?, true);

        Ok(())
    }
}

fn sample_test_hooks(shared_status: SharedStatus) -> TestFuncHooks {
    TestFuncHooks {
        get_a: Arc::new(|| 21),
        get_b: Arc::new(|| 2),
        print: Arc::new(move |value| {
            let mut shared_status = shared_status.try_lock().unwrap();
            shared_status.print_output = Some(value.to_string());
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
        let node: Node = func.into();
        let view_node: ViewNode = (&node).into();

        view_graph.view_nodes.add(view_node);
        view_graph.graph.add(node);
    }
}
