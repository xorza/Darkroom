use crate::common::undo_stack::{ActionUndoStack, UndoStack};
use crate::elements::editor_funclib::EditorFuncLib;
use crate::gui::graph_ui_interaction::{AutorunCommand, GraphUiAction, GraphUiInteraction};
use crate::model::config::Config;
use anyhow::Result;
use common::{FileFormat, Shared};
use graph::elements::timers_funclib::{FRAME_EVENT_FUNC_ID, TimersFuncLib};
use graph::execution_graph::Result as ExecutionGraphResult;
use graph::graph::{Binding, Node, NodeId};
use graph::prelude::{ExecutionStats, FuncId, FuncLib};
use graph::prelude::{TestFuncHooks, test_func_lib, test_graph};
use graph::worker::WorkerMessage;
use graph::worker::{EventRef, ProcessingCallback, Worker};
use std::path::Path;
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
    pub func_lib: FuncLib,
    pub view_graph: ViewGraph,
    pub interaction: GraphUiInteraction,
    pub execution_stats: Option<ExecutionStats>,

    pub status: String,

    pub config: Config,

    pub worker: Worker,

    pub _ui_context: UiContext,

    pub shared_status: SharedStatus,
    pub run_event: Arc<Notify>,
    pub autorun: bool,
    pub graph_dirty: bool,

    undo_stack: Box<dyn UndoStack<ViewGraph, Action = GraphUiAction>>,
}

impl AppData {
    pub fn new(ui_context: UiContext) -> Self {
        let config = Config::load_or_default();

        let shared_status = Shared::default();
        let worker = Self::create_worker(shared_status.clone(), ui_context.clone());

        let run_event = Arc::new(Notify::new());

        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(sample_test_hooks(shared_status.clone())));
        func_lib.merge(TimersFuncLib::default());
        func_lib.merge(EditorFuncLib::new());

        let mut result = Self {
            func_lib,
            view_graph: ViewGraph::default(),
            interaction: GraphUiInteraction::default(),
            execution_stats: None,

            status: String::new(),

            config,

            worker,

            _ui_context: ui_context,

            shared_status,
            run_event,
            autorun: false,
            graph_dirty: true,

            undo_stack: Box::new(ActionUndoStack::new(UNDO_MAX_STEPS)),
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
            let format = FileFormat::from_file_name(path.to_string_lossy().as_ref())
                .map_err(anyhow::Error::from)?;
            let payload = this.view_graph.serialize(format);
            std::fs::write(path, payload).map_err(anyhow::Error::from)
        }

        match save_to_file(self, path) {
            Ok(()) => {
                self.config.current_path = Some(path.to_path_buf());
                self.add_status(format!("Saved graph to {}", path.display()));
            }
            Err(err) => self.add_status(format!("Save failed: {err}")),
        }
    }

    pub fn load_graph(&mut self, path: &Path) {
        fn load_from_file(this: &mut AppData, path: &Path) -> Result<()> {
            let format = FileFormat::from_file_name(path.to_string_lossy().as_ref())
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
                self.add_status(format!("Load failed: {err}"));
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
                        format!("Compute output:\n{print_output}\n{summary}")
                    } else {
                        format!("Compute finished: {summary}")
                    };

                    self.status.push('\n');
                    self.status.push_str(&message);
                }
                Err(err) => {
                    self.status.push('\n');
                    self.status.push_str(&format!("Compute failed: {err}"));
                }
            }
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

    pub fn handle_interaction(&mut self) {
        while let Some(err) = self.interaction.errors.pop() {
            self.add_status(format!("Error: {err}"));
        }

        self.graph_dirty |= self.handle_actions();

        let mut msgs: Vec<WorkerMessage> = Vec::default();

        match self.interaction.autorun {
            AutorunCommand::Start => {
                if !self.autorun {
                    msgs.push(WorkerMessage::StartEventLoop);
                }
                self.autorun = true;
            }
            AutorunCommand::Stop => {
                msgs.push(WorkerMessage::StopEventLoop);
                self.autorun = false;
            }
            AutorunCommand::None => {}
        }

        let update_graph = self.graph_dirty && (self.autorun || self.interaction.run);

        if update_graph {
            msgs.push(WorkerMessage::Update {
                graph: self.view_graph.graph.clone(),
                func_lib: self.func_lib.clone(),
            });
            self.graph_dirty = false;
        }

        if self.interaction.run || (self.autorun && update_graph) {
            msgs.push(WorkerMessage::ExecuteTerminals);
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

    fn create_worker(shared_status: SharedStatus, ui_refresh: UiContext) -> Worker {
        Worker::new(move |result| {
            let mut shared_status = shared_status.try_lock().unwrap();
            shared_status.execution_stats = Some(result);

            ui_refresh.request_redraw();
        })
    }

    fn add_status(&mut self, message: impl AsRef<str>) {
        if !self.status.is_empty() {
            self.status.push('\n');
        }
        self.status.push_str(message.as_ref());
    }

    fn apply_graph(&mut self, view_graph: ViewGraph, reset_undo: bool) {
        self.view_graph = view_graph;

        if reset_undo {
            self.undo_stack.reset_with(&self.view_graph);
        }

        self.refresh_after_graph_change();
    }

    fn refresh_after_graph_change(&mut self) {
        self.graph_dirty = true;
        self.execution_stats = None;
    }

    fn handle_actions(&mut self) -> bool {
        let mut graph_updated = false;

        for actions in self.interaction.actions_stacks() {
            self.undo_stack.clear_redo();
            self.undo_stack.push_current(&self.view_graph, actions);

            if actions.iter().any(|action| action.affects_computation()) {
                self.execution_stats = None;
                graph_updated = true;
            }
        }

        graph_updated
    }
}

fn sample_test_hooks(shared_status: SharedStatus) -> TestFuncHooks {
    TestFuncHooks {
        get_a: Arc::new(|| 21),
        get_b: Arc::new(|| 2),
        print: Arc::new(move |value| {
            let mut shared_status = shared_status.try_lock().unwrap();
            if shared_status.print_output.is_none() {
                shared_status.print_output = Some(String::new());
            }

            let print_output = shared_status.print_output.as_mut().unwrap();
            if !print_output.is_empty() {
                print_output.push('\n');
            }
            print_output.push_str(&value.to_string());
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
