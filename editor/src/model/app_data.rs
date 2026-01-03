use arc_swap::ArcSwapOption;
use eframe::egui;
use graph::prelude::FuncLib;
use graph::worker::Worker;
use std::path::PathBuf;
use std::sync::Arc;

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
}

impl AppData {
    pub fn new(ui_context: &egui::Context, current_path: PathBuf) -> Self {
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
        }
    }

    fn create_worker(
        updated_status: &Arc<ArcSwapOption<String>>,
        print_output: &Arc<ArcSwapOption<String>>,
        ui_context: egui::Context,
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

            ui_context.request_repaint();
        })
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
    }

    pub fn poll_compute_status(&mut self) {
        let updated_status = self.updated_status.swap(None);
        if let Some(updated_status) = updated_status {
            self.set_status(updated_status.as_ref());
        }
    }
}
