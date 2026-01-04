use crate::gui::graph_ui::GraphUiInteraction;
use anyhow::Result;
use arc_swap::ArcSwapOption;
use common::FileFormat;
use graph::graph::Binding;
use graph::prelude::FuncLib;
use graph::prelude::{TestFuncHooks, test_func_lib, test_graph};
use graph::worker::Worker;
use graph::worker::WorkerMessage;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::main_ui::UiContext;
use crate::model::ViewGraph;

#[derive(Debug)]
pub struct AppData {
    pub worker: Worker,
    pub func_lib: FuncLib,
    pub view_graph: ViewGraph,
    pub graph_updated: bool,
    pub current_path: PathBuf,
    pub print_output: Arc<ArcSwapOption<String>>,
    pub updated_status: Arc<ArcSwapOption<String>>,
    pub status: String,
    pub ui_context: UiContext,
}

impl AppData {
    pub fn new(ui_context: UiContext, current_path: PathBuf) -> Self {
        let updated_status: Arc<ArcSwapOption<String>> = Arc::new(ArcSwapOption::empty());
        let print_output: Arc<ArcSwapOption<String>> = Arc::new(ArcSwapOption::empty());
        let worker = Self::create_worker(&updated_status, &print_output, ui_context.clone());

        Self {
            worker,
            func_lib: FuncLib::default(),
            view_graph: ViewGraph::default(),
            graph_updated: false,
            current_path,
            print_output,
            updated_status,
            status: String::new(),
            ui_context,
        }
    }

    fn create_worker(
        updated_status: &Arc<ArcSwapOption<String>>,
        print_output: &Arc<ArcSwapOption<String>>,
        ui_refresh: UiContext,
    ) -> Worker {
        let updated_status = Arc::clone(updated_status);
        let print_output = Arc::clone(print_output);

        Worker::new(move |result| {
            match result {
                Ok(stats) => {
                    let print_output = print_output.swap(None);
                    let summary = format!(
                        "({} nodes, {:.0}s)",
                        stats.executed_nodes, stats.elapsed_secs
                    );
                    let message = if let Some(print_output) = print_output {
                        format!("Compute output: {print_output} {summary}")
                    } else {
                        format!("Compute finished {summary}")
                    };
                    updated_status.store(Some(Arc::new(message)));
                }
                Err(err) => {
                    updated_status.store(Some(Arc::new(format!("Compute failed: {err}"))));
                }
            }

            ui_refresh.request_redraw();
        })
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
    }

    pub fn save_graph(&mut self) {
        match self.save_to_file(&self.view_graph, &self.current_path) {
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

    pub fn apply_graph(&mut self, view_graph: ViewGraph) {
        view_graph
            .validate()
            .expect("graph should be valid before storing in app state");

        self.view_graph = view_graph;
        self.worker.send(WorkerMessage::Clear);
        self.graph_updated = true;
    }

    pub fn handle_graph_ui_actions(&mut self, graph_ui_interaction: &GraphUiInteraction) {
        if graph_ui_interaction.actions.is_empty() {
            return;
        }

        self.graph_updated = true;
    }

    pub fn empty_graph(&mut self) {
        self.apply_graph(ViewGraph::default());
        self.set_status("Created new graph");
    }

    pub fn load_test_graph(&mut self) {
        let mut graph = test_graph();
        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::Const(132.into());

        self.func_lib = test_func_lib(Self::sample_test_hooks(self));
        let graph_view = ViewGraph::from_graph(&graph);
        self.apply_graph(graph_view);

        self.set_status("Loaded sample test graph");
    }

    fn sample_test_hooks(app_data: &AppData) -> TestFuncHooks {
        let print_output = Arc::clone(&app_data.print_output);
        TestFuncHooks {
            get_a: Arc::new(|| 21),
            get_b: Arc::new(|| 2),
            print: Arc::new(move |value| {
                print_output.store(Some(Arc::new(value.to_string())));
            }),
        }
    }

    pub fn pre_render_update(&mut self) {
        let updated_status = self.updated_status.swap(None);
        if let Some(updated_status) = updated_status {
            self.set_status(updated_status.as_ref());
        }
    }

    fn save_to_file(&self, graph: &ViewGraph, path: &Path) -> Result<()> {
        let format = FileFormat::from_file_name(path.to_string_lossy().as_ref())
            .map_err(anyhow::Error::from)?;
        let payload = graph.serialize(format);
        std::fs::write(path, payload).map_err(anyhow::Error::from)
    }

    fn load_from_file(&mut self, path: &Path) -> Result<()> {
        let format = FileFormat::from_file_name(path.to_string_lossy().as_ref())
            .map_err(anyhow::Error::from)?;
        let payload = std::fs::read_to_string(path).map_err(anyhow::Error::from)?;
        self.apply_graph(ViewGraph::deserialize(format, &payload)?);
        Ok(())
    }
}
