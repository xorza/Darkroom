use crate::gui::graph_ui_interaction::GraphUiInteraction;
use anyhow::Result;
use common::{FileFormat, Shared};
use graph::execution_graph::Result as ExecutionGraphResult;
use graph::graph::Binding;
use graph::prelude::{ExecutionStats, FuncLib};
use graph::prelude::{TestFuncHooks, test_func_lib, test_graph};
use graph::worker::Worker;
use graph::worker::WorkerMessage;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::main_ui::UiContext;
use crate::model::ViewGraph;

#[derive(Debug, Default)]
pub struct Status {
    execution_stats: Option<ExecutionGraphResult<ExecutionStats>>,
    print_output: Option<String>,
}

pub type SharedStatus = Shared<Status>;

const UNDO_FILE_FORMAT: FileFormat = FileFormat::Lua;

#[derive(Debug)]
pub struct AppData {
    pub worker: Worker,
    pub func_lib: FuncLib,
    pub view_graph: ViewGraph,
    pub execution_stats: Option<ExecutionStats>,
    pub graph_updated: bool,
    pub current_path: PathBuf,
    pub status: String,
    pub _ui_context: UiContext,

    pub shared_status: SharedStatus,

    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
}

impl AppData {
    pub fn new(ui_context: UiContext, current_path: PathBuf) -> Self {
        let shared_status = Shared::default();

        let worker = Self::create_worker(shared_status.clone(), ui_context.clone());

        Self {
            worker,
            func_lib: FuncLib::default(),
            view_graph: ViewGraph::default(),
            execution_stats: None,
            graph_updated: false,
            current_path,
            status: String::new(),
            _ui_context: ui_context,

            shared_status,

            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
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
            self.graph_updated = false;
        }
        self.worker.event();
    }

    pub fn undo(&mut self) {
        if self.undo_stack.len() < 2 {
            return;
        }

        let current = self.undo_stack.pop().unwrap();
        self.redo_stack.push(current);

        let snapshot = self
            .undo_stack
            .last()
            .expect("undo stack should contain a prior snapshot");
        self.apply_graph(
            ViewGraph::deserialize(UNDO_FILE_FORMAT, snapshot)
                .expect("Failed to deserialize undo snapshot"),
            false,
        );

        tracing::info!("Undo applied, stack size: {}", self.undo_stack.len());
    }

    pub fn redo(&mut self) {
        if self.redo_stack.is_empty() {
            return;
        }

        let snapshot = self
            .redo_stack
            .pop()
            .expect("redo stack should contain a snapshot when redo is requested");

        self.undo_stack.push(snapshot.clone());
        self.apply_graph(
            ViewGraph::deserialize(UNDO_FILE_FORMAT, &snapshot)
                .expect("Failed to deserialize redo snapshot"),
            false,
        );
    }

    fn push_undo(&mut self) {
        let snapshot = self.view_graph.serialize(UNDO_FILE_FORMAT);
        if self.undo_stack.last().is_some_and(|last| last == &snapshot) {
            println!("skip");
            return;
        }
        self.undo_stack.push(snapshot);

        tracing::info!("Undo added, stack size: {}", self.undo_stack.len());
    }

    pub fn apply_graph(&mut self, view_graph: ViewGraph, reset_undo: bool) {
        view_graph.validate();

        self.view_graph = view_graph;
        self.worker.send(WorkerMessage::Clear);
        self.graph_updated = true;
        self.execution_stats = None;

        if reset_undo {
            self.undo_stack.clear();
            self.redo_stack.clear();
            self.push_undo();
        }
    }

    pub fn handle_graph_ui_actions(&mut self, graph_ui_interaction: &GraphUiInteraction) {
        if graph_ui_interaction
            .actions
            .iter()
            .any(|action| action.affects_computation())
        {
            self.redo_stack.clear();
            self.execution_stats = None;
            self.graph_updated = true;

            self.push_undo();
        }

        if graph_ui_interaction.run {
            self.run_graph();
        }

        if let Some(err) = graph_ui_interaction.errors.last() {
            self.set_status(format!("Graph error: {err}"));
        }
    }

    pub fn empty_graph(&mut self) {
        self.apply_graph(ViewGraph::default(), true);
        self.set_status("Created new graph");
    }

    pub fn load_test_graph(&mut self) {
        let mut graph = test_graph();
        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::Const(132.into());
        graph.by_name_mut("sum").unwrap().inputs[1].binding = Binding::Const(22455.into());

        self.func_lib = test_func_lib(Self::sample_test_hooks(self));
        let graph_view = ViewGraph::from_graph(&graph);
        self.apply_graph(graph_view, true);

        self.set_status("Loaded sample test graph");
    }

    fn sample_test_hooks(app_data: &AppData) -> TestFuncHooks {
        let shared_status = app_data.shared_status.clone();
        TestFuncHooks {
            get_a: Arc::new(|| 21),
            get_b: Arc::new(|| 2),
            print: Arc::new(move |value| {
                let mut shared_status = shared_status.try_lock().unwrap();
                shared_status.print_output = Some(value.to_string());
            }),
        }
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
        let payload = std::fs::read_to_string(path).map_err(anyhow::Error::from)?;
        self.apply_graph(ViewGraph::deserialize(format, &payload)?, true);

        Ok(())
    }
}
